//! OAuth 2.0 authentication module for MCP Streamable HTTP transport.
//!
//! Provides a complete OAuth flow for authenticating with MCP servers that
//! require OAuth 2.0 authorization. Implements:
//!
//! - **Token storage**: File-based persistence of OAuth tokens with automatic
//!   expiry detection and refresh-skew handling.
//! - **OAuth discovery**: RFC 8414 well-known endpoint discovery for
//!   authorization server metadata.
//! - **Dynamic client registration**: RFC 7591 dynamic client registration
//!   for MCP servers that support it.
//! - **PKCE login flow**: Full authorization code flow with PKCE (S256),
//!   local callback server, and browser-based authorization.
//! - **Token refresh**: Automatic token refresh using refresh_token grant.
//! - **Auth status management**: Unified authentication state tracking with
//!   auto-refresh capability.
//! - **AuthProvider**: High-level provider that resolves authentication from
//!   config (env var bearer token, stored OAuth tokens, or login required).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use url::Url;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How many seconds before actual expiry we consider a token "needs refresh".
const REFRESH_SKEW_SECS: u64 = 30;

/// Timeout for OAuth discovery HTTP requests.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout waiting for the local callback server to receive the auth code.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// Subdirectory under data_dir for OAuth token storage.
const OAUTH_STORAGE_DIR: &str = "oauth";

/// Keyring service name for OAuth tokens.
#[cfg(feature = "keyring-store")]
const KEYRING_SERVICE: &str = "bifrost-agent-oauth";

// ---------------------------------------------------------------------------
// Credentials Store Mode
// ---------------------------------------------------------------------------

/// Storage mode for OAuth credentials.
///
/// Matches Codex's `OAuthCredentialsStoreMode` with Auto/File/Keyring/Ephemeral modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OAuthCredentialsStoreMode {
    /// Automatically choose the best available storage:
    /// keyring if available, file as fallback.
    #[default]
    Auto,
    /// Always use file-based storage (CODEX_HOME/.credentials.json).
    /// Readable by other applications running as the same user.
    File,
    /// Always use OS keychain (macOS Keychain, Windows Credential Manager, etc.).
    /// Requires the `keyring-store` feature.
    Keyring,
    /// Store credentials in memory only for the current process.
    /// Tokens are lost when the process exits.
    Ephemeral,
}

// ---------------------------------------------------------------------------
// Token Storage Types
// ---------------------------------------------------------------------------

/// Stored OAuth tokens for an MCP server, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredOAuthTokens {
    pub server_name: String,
    pub url: String,
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    /// Unix timestamp (seconds) when the access token expires.
    pub expires_at: Option<u64>,
    pub scope: Option<String>,
}

/// Token response from the authorization server's token endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

// ---------------------------------------------------------------------------
// OAuth Token Store
// ---------------------------------------------------------------------------

/// File-based OAuth token store.
///
/// Tokens are stored at `{data_dir}/oauth/{server_name}.json` with
/// filesystem permissions restricted to the current user (0o600 on Unix).
pub struct OAuthTokenStore {
    data_dir: PathBuf,
}

impl OAuthTokenStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Storage path for a given server's tokens.
    fn token_path(&self, server_name: &str) -> PathBuf {
        self.data_dir
            .join(OAUTH_STORAGE_DIR)
            .join(format!("{}.json", sanitize_filename(server_name)))
    }

    /// Load stored tokens for a server. Returns `None` if no tokens exist
    /// or the file cannot be read.
    pub fn load(&self, server_name: &str) -> anyhow::Result<Option<StoredOAuthTokens>> {
        let path = self.token_path(server_name);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let tokens: StoredOAuthTokens = serde_json::from_str(&content)
                    .map_err(|e| anyhow::anyhow!("failed to parse OAuth tokens: {e}"))?;
                debug!(
                    server = %server_name,
                    path = %path.display(),
                    "loaded OAuth tokens from file"
                );
                Ok(Some(tokens))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::anyhow!(
                "failed to read OAuth token file {}: {e}",
                path.display()
            )),
        }
    }

    /// Save tokens to the store.
    pub fn save(&self, tokens: &StoredOAuthTokens) -> anyhow::Result<()> {
        let path = self.token_path(&tokens.server_name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("failed to create OAuth storage dir: {e}"))?;
        }

        let json = serde_json::to_string_pretty(tokens)
            .map_err(|e| anyhow::anyhow!("failed to serialize OAuth tokens: {e}"))?;
        std::fs::write(&path, &json)
            .map_err(|e| anyhow::anyhow!("failed to write OAuth tokens: {e}"))?;

        // Restrict file permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }

        debug!(
            server = %tokens.server_name,
            path = %path.display(),
            "saved OAuth tokens to file"
        );
        Ok(())
    }

    /// Delete stored tokens for a server.
    pub fn delete(&self, server_name: &str) -> anyhow::Result<bool> {
        let path = self.token_path(server_name);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                debug!(server = %server_name, "deleted OAuth tokens");
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(anyhow::anyhow!("failed to delete OAuth token file: {e}")),
        }
    }

    /// Check if a stored token is expired or about to expire (within REFRESH_SKEW_SECS).
    pub fn needs_refresh(tokens: &StoredOAuthTokens) -> bool {
        let Some(expires_at) = tokens.expires_at else {
            // No expiry info → assume still valid (server may not provide expires_in)
            return false;
        };
        let now = current_unix_secs();
        now.saturating_add(REFRESH_SKEW_SECS) >= expires_at
    }
}

// ---------------------------------------------------------------------------
// Keyring-backed Token Store
// ---------------------------------------------------------------------------

/// Store/load/delete OAuth tokens via OS keychain.
///
/// Uses the `keyring` crate to access platform-native credential storage:
/// - macOS: Keychain
/// - Windows: Credential Manager
/// - Linux: DBus Secret Service
///
/// Matching Codex's `codex_keyring_store` pattern. Storage key follows
/// Codex's `compute_store_key` format: `"{server_name}|{sha256_prefix_16}"`.
#[cfg(feature = "keyring-store")]
pub struct KeyringTokenStore;

#[cfg(feature = "keyring-store")]
impl KeyringTokenStore {
    /// Compute the keyring entry username from server name and URL.
    ///
    /// Matches Codex's `compute_store_key` algorithm exactly:
    /// 1. Build JSON payload: `{"type":"http","url":"<url>","headers":{}}`
    /// 2. SHA-256 the serialized JSON string
    /// 3. Take first 16 hex chars (8 bytes)
    /// 4. Format as `"{server_name}|{prefix}"`
    fn compute_key(server_name: &str, url: &str) -> String {
        // Build the same JSON payload structure as Codex
        let payload = serde_json::json!({
            "type": "http",
            "url": url,
            "headers": {}
        });
        let serialized = serde_json::to_string(&payload).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        let hash = hasher.finalize();
        // Take first 8 bytes (16 hex chars), matching Codex's sha_256_prefix
        let prefix: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
        format!("{server_name}|{prefix}")
    }

    /// Load tokens from OS keychain.
    pub fn load(server_name: &str, url: &str) -> anyhow::Result<Option<StoredOAuthTokens>> {
        let key = Self::compute_key(server_name, url);
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
            .map_err(|e| anyhow::anyhow!("keyring entry creation failed: {e}"))?;

        match entry.get_password() {
            Ok(json) => {
                let tokens: StoredOAuthTokens = serde_json::from_str(&json)
                    .map_err(|e| anyhow::anyhow!("failed to parse keyring OAuth tokens: {e}"))?;
                debug!(server = %server_name, "loaded OAuth tokens from keyring");
                Ok(Some(tokens))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("keyring load failed: {e}")),
        }
    }

    /// Save tokens to OS keychain.
    pub fn save(tokens: &StoredOAuthTokens) -> anyhow::Result<()> {
        let key = Self::compute_key(&tokens.server_name, &tokens.url);
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
            .map_err(|e| anyhow::anyhow!("keyring entry creation failed: {e}"))?;

        let json = serde_json::to_string(tokens)
            .map_err(|e| anyhow::anyhow!("failed to serialize OAuth tokens: {e}"))?;

        entry
            .set_password(&json)
            .map_err(|e| anyhow::anyhow!("keyring save failed: {e}"))?;

        debug!(server = %tokens.server_name, "saved OAuth tokens to keyring");
        Ok(())
    }

    /// Delete tokens from OS keychain.
    pub fn delete(server_name: &str, url: &str) -> anyhow::Result<bool> {
        let key = Self::compute_key(server_name, url);
        let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
            .map_err(|e| anyhow::anyhow!("keyring entry creation failed: {e}"))?;

        match entry.delete_credential() {
            Ok(()) => {
                debug!(server = %server_name, "deleted OAuth tokens from keyring");
                Ok(true)
            }
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(anyhow::anyhow!("keyring delete failed: {e}")),
        }
    }

    /// Check if keyring is available on this system.
    pub fn is_available() -> bool {
        // Entry creation can succeed even when the platform backend cannot
        // actually persist/read credentials (for example headless Linux CI).
        let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, "__bifrost_probe__") else {
            return false;
        };
        let probe = "__bifrost_keyring_available__";
        if entry.set_password(probe).is_err() {
            return false;
        }
        let available = entry.get_password().is_ok_and(|value| value == probe);
        let _ = entry.delete_credential();
        available
    }
}

/// Save tokens using the specified storage mode.
pub fn save_oauth_tokens_with_mode(
    tokens: &StoredOAuthTokens,
    data_dir: &Path,
    mode: OAuthCredentialsStoreMode,
) -> anyhow::Result<()> {
    match mode {
        OAuthCredentialsStoreMode::Ephemeral => {
            // Ephemeral mode: tokens are only held in memory, never persisted.
            Ok(())
        }
        OAuthCredentialsStoreMode::File => OAuthTokenStore::new(data_dir).save(tokens),
        #[cfg(feature = "keyring-store")]
        OAuthCredentialsStoreMode::Keyring => KeyringTokenStore::save(tokens),
        #[cfg(not(feature = "keyring-store"))]
        OAuthCredentialsStoreMode::Keyring => {
            warn!("keyring-store feature not enabled, falling back to file storage");
            OAuthTokenStore::new(data_dir).save(tokens)
        }
        OAuthCredentialsStoreMode::Auto => {
            #[cfg(feature = "keyring-store")]
            if KeyringTokenStore::is_available() {
                match KeyringTokenStore::save(tokens)
                    .and_then(|_| KeyringTokenStore::load(&tokens.server_name, &tokens.url))
                {
                    Ok(Some(loaded)) if loaded.access_token == tokens.access_token => {
                        return Ok(());
                    }
                    Ok(_) => {
                        warn!("keyring save verification failed, falling back to file storage");
                    }
                    Err(error) => {
                        warn!(error = %error, "keyring save failed, falling back to file storage");
                    }
                }
            }
            OAuthTokenStore::new(data_dir).save(tokens)
        }
    }
}

/// Load tokens using the specified storage mode.
#[allow(unused_variables)]
pub fn load_oauth_tokens_with_mode(
    server_name: &str,
    url: &str,
    data_dir: &Path,
    mode: OAuthCredentialsStoreMode,
) -> anyhow::Result<Option<StoredOAuthTokens>> {
    match mode {
        OAuthCredentialsStoreMode::Ephemeral => {
            // Ephemeral mode: nothing is persisted, always return None.
            Ok(None)
        }
        OAuthCredentialsStoreMode::File => OAuthTokenStore::new(data_dir).load(server_name),
        #[cfg(feature = "keyring-store")]
        OAuthCredentialsStoreMode::Keyring => KeyringTokenStore::load(server_name, url),
        #[cfg(not(feature = "keyring-store"))]
        OAuthCredentialsStoreMode::Keyring => {
            warn!("keyring-store feature not enabled, falling back to file storage");
            OAuthTokenStore::new(data_dir).load(server_name)
        }
        OAuthCredentialsStoreMode::Auto => {
            // Try keyring first, fall back to file
            #[cfg(feature = "keyring-store")]
            if KeyringTokenStore::is_available() {
                if let Ok(Some(tokens)) = KeyringTokenStore::load(server_name, url) {
                    return Ok(Some(tokens));
                }
            }
            OAuthTokenStore::new(data_dir).load(server_name)
        }
    }
}

/// Delete tokens using the specified storage mode.
///
/// Matches Codex's `delete_oauth_tokens` which removes from both keyring and file
/// to ensure no stale credentials remain.
#[allow(unused_variables)]
pub fn delete_oauth_tokens_with_mode(
    server_name: &str,
    url: &str,
    data_dir: &Path,
    mode: OAuthCredentialsStoreMode,
) -> anyhow::Result<bool> {
    match mode {
        OAuthCredentialsStoreMode::Ephemeral => Ok(false),
        OAuthCredentialsStoreMode::File => OAuthTokenStore::new(data_dir).delete(server_name),
        #[cfg(feature = "keyring-store")]
        OAuthCredentialsStoreMode::Keyring => KeyringTokenStore::delete(server_name, url),
        #[cfg(not(feature = "keyring-store"))]
        OAuthCredentialsStoreMode::Keyring => OAuthTokenStore::new(data_dir).delete(server_name),
        OAuthCredentialsStoreMode::Auto => {
            // Delete from both keyring and file to match Codex behavior
            let mut removed = false;
            #[cfg(feature = "keyring-store")]
            if KeyringTokenStore::is_available() {
                if let Ok(true) = KeyringTokenStore::delete(server_name, url) {
                    removed = true;
                }
            }
            if let Ok(true) = OAuthTokenStore::new(data_dir).delete(server_name) {
                removed = true;
            }
            Ok(removed)
        }
    }
}

// ---------------------------------------------------------------------------
// OAuth Server Metadata (RFC 8414)
// ---------------------------------------------------------------------------

/// OAuth 2.0 Authorization Server Metadata (RFC 8414).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthServerMetadata {
    #[serde(default)]
    pub issuer: Option<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub response_types_supported: Option<Vec<String>>,
    #[serde(default)]
    pub grant_types_supported: Option<Vec<String>>,
    #[serde(default)]
    pub code_challenge_methods_supported: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// OAuth Discovery
// ---------------------------------------------------------------------------

/// Discover OAuth server metadata via RFC 8414 well-known endpoint.
///
/// Tries `{base_url}/.well-known/oauth-authorization-server` with a 5s timeout.
/// Returns `None` if the server does not advertise OAuth metadata.
pub async fn discover_oauth_metadata(
    base_url: &str,
) -> anyhow::Result<Option<OAuthServerMetadata>> {
    let parsed =
        Url::parse(base_url).map_err(|e| anyhow::anyhow!("invalid base URL '{base_url}': {e}"))?;

    let client = bifrost_core::outbound_reqwest_client_builder()
        .timeout(DISCOVERY_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    // Try discovery paths per RFC 8414 section 3.1
    for candidate in discovery_paths(parsed.path()) {
        let mut discovery_url = parsed.clone();
        discovery_url.set_path(&candidate);

        debug!(url = %discovery_url, "attempting OAuth discovery");

        let resp = match client.get(discovery_url.as_str()).send().await {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, "OAuth discovery request failed");
                continue;
            }
        };

        if !resp.status().is_success() {
            debug!(status = %resp.status(), "OAuth discovery returned non-success");
            continue;
        }

        match resp.json::<OAuthServerMetadata>().await {
            Ok(metadata) => {
                info!(
                    authorization_endpoint = %metadata.authorization_endpoint,
                    token_endpoint = %metadata.token_endpoint,
                    "discovered OAuth server metadata"
                );
                return Ok(Some(metadata));
            }
            Err(e) => {
                debug!(error = %e, "failed to parse OAuth discovery response");
                continue;
            }
        }
    }

    Ok(None)
}

/// Generate discovery paths per RFC 8414 section 3.1.
fn discovery_paths(base_path: &str) -> Vec<String> {
    let trimmed = base_path.trim_start_matches('/').trim_end_matches('/');
    let canonical = "/.well-known/oauth-authorization-server".to_string();

    if trimmed.is_empty() {
        return vec![canonical];
    }

    let mut candidates = Vec::new();
    let with_path = format!("{canonical}/{trimmed}");
    if !candidates.contains(&with_path) {
        candidates.push(with_path);
    }
    let path_prefix = format!("/{trimmed}/.well-known/oauth-authorization-server");
    if !candidates.contains(&path_prefix) {
        candidates.push(path_prefix);
    }
    if !candidates.contains(&canonical) {
        candidates.push(canonical);
    }
    candidates
}

// ---------------------------------------------------------------------------
// Dynamic Client Registration (RFC 7591)
// ---------------------------------------------------------------------------

/// Registration request sent to the server's registration endpoint.
#[derive(Debug, Serialize)]
struct RegistrationRequest {
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    token_endpoint_auth_method: String,
}

/// Registration response from the server.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistrationResponse {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub redirect_uris: Vec<String>,
}

/// Perform dynamic client registration with an OAuth server.
pub async fn register_client(
    registration_endpoint: &str,
    client_name: &str,
    redirect_uri: &str,
) -> anyhow::Result<RegistrationResponse> {
    let client = bifrost_core::outbound_reqwest_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    let request = RegistrationRequest {
        client_name: client_name.to_string(),
        redirect_uris: vec![redirect_uri.to_string()],
        grant_types: vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ],
        response_types: vec!["code".to_string()],
        token_endpoint_auth_method: "none".to_string(),
    };

    debug!(
        endpoint = %registration_endpoint,
        client_name = %client_name,
        "registering OAuth client"
    );

    let resp = client
        .post(registration_endpoint)
        .json(&request)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("client registration request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "client registration failed with status {status}: {body}"
        ));
    }

    let registration: RegistrationResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("failed to parse registration response: {e}"))?;

    info!(client_id = %registration.client_id, "OAuth client registered");
    Ok(registration)
}

// ---------------------------------------------------------------------------
// PKCE (Proof Key for Code Exchange)
// ---------------------------------------------------------------------------

/// PKCE pair: code_verifier + code_challenge (S256).
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generate a PKCE challenge pair using S256 method.
///
/// The code_verifier is a 32-byte random value encoded as URL-safe base64
/// (43 characters). The code_challenge is the SHA-256 hash of the verifier,
/// also URL-safe base64 encoded.
pub fn generate_pkce_challenge() -> PkceChallenge {
    let mut rng = rand::thread_rng();
    let mut verifier_bytes = [0u8; 32];
    rng.fill(&mut verifier_bytes);

    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let hash = hasher.finalize();
    let code_challenge = URL_SAFE_NO_PAD.encode(hash);

    PkceChallenge {
        code_verifier,
        code_challenge,
    }
}

// ---------------------------------------------------------------------------
// OAuth Login Flow
// ---------------------------------------------------------------------------

/// Manages the complete OAuth authorization code flow with PKCE.
pub struct OAuthLoginFlow {
    server_name: String,
    base_url: String,
    scopes: Vec<String>,
    oauth_resource: Option<String>,
    data_dir: PathBuf,
}

impl OAuthLoginFlow {
    pub fn new(
        server_name: &str,
        base_url: &str,
        scopes: Vec<String>,
        oauth_resource: Option<String>,
        data_dir: &Path,
    ) -> Self {
        Self {
            server_name: server_name.to_string(),
            base_url: base_url.to_string(),
            scopes,
            oauth_resource,
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Execute the full OAuth login flow:
    /// 1. Discover OAuth metadata
    /// 2. Register client (if registration endpoint available)
    /// 3. Generate PKCE challenge
    /// 4. Start local callback server
    /// 5. Open browser for authorization
    /// 6. Wait for callback with authorization code
    /// 7. Exchange code for tokens
    /// 8. Save tokens
    pub async fn execute(&self) -> anyhow::Result<StoredOAuthTokens> {
        info!(
            server = %self.server_name,
            url = %self.base_url,
            "starting OAuth login flow"
        );

        // Step 1: Discover OAuth metadata
        let metadata = discover_oauth_metadata(&self.base_url)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "OAuth discovery failed: server '{}' does not advertise OAuth metadata",
                    self.server_name
                )
            })?;

        // Step 2: Register client if endpoint available
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| anyhow::anyhow!("failed to bind callback server: {e}"))?;
        let callback_port = listener
            .local_addr()
            .map_err(|e| anyhow::anyhow!("failed to get callback port: {e}"))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{callback_port}/callback");

        let client_id = if let Some(ref reg_endpoint) = metadata.registration_endpoint {
            let reg = register_client(reg_endpoint, "bifrost-agent", &redirect_uri).await?;
            reg.client_id
        } else {
            // Use a default client_id if no registration endpoint
            "bifrost-agent".to_string()
        };

        // Step 3: Generate PKCE challenge
        let pkce = generate_pkce_challenge();

        // Step 4: Build authorization URL
        let state = generate_state();
        let auth_url = build_authorization_url(
            &metadata.authorization_endpoint,
            &client_id,
            &redirect_uri,
            &self.scopes,
            &state,
            &pkce.code_challenge,
            self.oauth_resource.as_deref(),
        )?;

        // Step 5: Open browser
        info!(url = %auth_url, "opening browser for OAuth authorization");
        if let Err(e) = open::that(&auth_url) {
            warn!(error = %e, "failed to open browser automatically");
            info!("Please open the following URL in your browser to authorize:\n{auth_url}");
        }

        // Step 6: Wait for callback
        let callback_result = wait_for_callback(listener, &state).await?;
        info!("received OAuth authorization code");

        // Step 7: Exchange code for tokens
        let token_response = exchange_code_for_token(
            &metadata.token_endpoint,
            &client_id,
            &callback_result.code,
            &redirect_uri,
            &pkce.code_verifier,
        )
        .await?;

        // Step 8: Build and save stored tokens
        let now = current_unix_secs();
        let expires_at = token_response.expires_in.map(|ei| now + ei);

        let stored = StoredOAuthTokens {
            server_name: self.server_name.clone(),
            url: self.base_url.clone(),
            client_id,
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            token_type: token_response.token_type,
            expires_at,
            scope: token_response.scope,
        };

        let store = OAuthTokenStore::new(&self.data_dir);
        store.save(&stored)?;

        info!(server = %self.server_name, "OAuth login completed successfully");
        Ok(stored)
    }
}

/// Build the full authorization URL with all required parameters.
fn build_authorization_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    state: &str,
    code_challenge: &str,
    resource: Option<&str>,
) -> anyhow::Result<String> {
    let mut url = Url::parse(authorization_endpoint)
        .map_err(|e| anyhow::anyhow!("invalid authorization endpoint: {e}"))?;

    {
        let mut params = url.query_pairs_mut();
        params.append_pair("response_type", "code");
        params.append_pair("client_id", client_id);
        params.append_pair("redirect_uri", redirect_uri);
        params.append_pair("state", state);
        params.append_pair("code_challenge", code_challenge);
        params.append_pair("code_challenge_method", "S256");

        if !scopes.is_empty() {
            params.append_pair("scope", &scopes.join(" "));
        }
        if let Some(res) = resource {
            if !res.trim().is_empty() {
                params.append_pair("resource", res);
            }
        }
    }

    Ok(url.to_string())
}

/// Generate a random state parameter for CSRF protection.
fn generate_state() -> String {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Result from the OAuth callback.
#[derive(Debug)]
struct CallbackResult {
    code: String,
}

/// Start a local HTTP server and wait for the OAuth callback.
async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> anyhow::Result<CallbackResult> {
    use tokio::io::AsyncReadExt as TokioAsyncReadExt;
    use tokio::io::AsyncWriteExt as TokioAsyncWriteExt;

    let expected_state = expected_state.to_string();

    let result = tokio::time::timeout(CALLBACK_TIMEOUT, async {
        loop {
            let (mut stream, _addr) = listener
                .accept()
                .await
                .map_err(|e| anyhow::anyhow!("callback server accept failed: {e}"))?;

            let mut buf = vec![0u8; 4096];
            let n = stream
                .read(&mut buf)
                .await
                .map_err(|e| anyhow::anyhow!("failed to read from callback connection: {e}"))?;

            let request = String::from_utf8_lossy(&buf[..n]);

            // Parse the request line to get the path
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("");

            // Parse query parameters from the callback
            if let Some(query_start) = path.find('?') {
                let query = &path[query_start + 1..];
                let mut code = None;
                let mut state = None;
                let mut error = None;
                let mut error_description = None;

                for pair in query.split('&') {
                    if let Some((key, value)) = pair.split_once('=') {
                        let decoded = urlencoding_decode(value);
                        match key {
                            "code" => code = Some(decoded),
                            "state" => state = Some(decoded),
                            "error" => error = Some(decoded),
                            "error_description" => error_description = Some(decoded),
                            _ => {}
                        }
                    }
                }

                // Check for error response
                if let Some(err) = error {
                    let desc = error_description.unwrap_or_default();
                    let response = format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\r\nOAuth error: {err}: {desc}"
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    return Err(anyhow::anyhow!(
                        "OAuth provider returned error '{err}': {desc}"
                    ));
                }

                // Validate state and extract code
                if let (Some(code), Some(state)) = (code, state) {
                    if state != expected_state {
                        let response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\n\r\nState mismatch";
                        let _ = stream.write_all(response.as_bytes()).await;
                        return Err(anyhow::anyhow!(
                            "OAuth state mismatch: expected '{}', got '{}'",
                            expected_state,
                            state
                        ));
                    }

                    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<html><body><h1>Authentication successful!</h1><p>You can close this window.</p></body></html>";
                    let _ = stream.write_all(response.as_bytes()).await;
                    return Ok(CallbackResult { code });
                }
            }

            // Not a valid callback request, send 404 and keep listening
            let response = "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\r\nNot Found";
            let _ = stream.write_all(response.as_bytes()).await;
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err(anyhow::anyhow!(
            "timed out waiting for OAuth callback ({}s)",
            CALLBACK_TIMEOUT.as_secs()
        )),
    }
}

/// Exchange an authorization code for tokens at the token endpoint.
async fn exchange_code_for_token(
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> anyhow::Result<TokenResponse> {
    let client = bifrost_core::outbound_reqwest_client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
    ];

    debug!(endpoint = %token_endpoint, "exchanging authorization code for tokens");

    let resp = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("token exchange request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "token exchange failed with status {status}: {body}"
        ));
    }

    let token_response: TokenResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("failed to parse token response: {e}"))?;

    debug!("token exchange successful");
    Ok(token_response)
}

// ---------------------------------------------------------------------------
// Token Refresh
// ---------------------------------------------------------------------------

/// Handles refreshing expired OAuth tokens.
pub struct OAuthTokenRefresher {
    token_endpoint: String,
    client_id: String,
}

impl OAuthTokenRefresher {
    pub fn new(token_endpoint: &str, client_id: &str) -> Self {
        Self {
            token_endpoint: token_endpoint.to_string(),
            client_id: client_id.to_string(),
        }
    }

    /// Refresh an access token using a refresh_token.
    pub async fn refresh(&self, refresh_token: &str) -> anyhow::Result<TokenResponse> {
        let client = bifrost_core::outbound_reqwest_client_builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", self.client_id.as_str()),
            ("refresh_token", refresh_token),
        ];

        debug!(
            endpoint = %self.token_endpoint,
            client_id = %self.client_id,
            "refreshing OAuth token"
        );

        let resp = client
            .post(&self.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("token refresh request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "token refresh failed with status {status}: {body}"
            ));
        }

        let token_response: TokenResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("failed to parse refresh token response: {e}"))?;

        info!("OAuth token refreshed successfully");
        Ok(token_response)
    }
}

// ---------------------------------------------------------------------------
// Auth Status
// ---------------------------------------------------------------------------

/// Authentication status for an MCP server connection.
#[derive(Debug, Clone)]
pub enum AuthStatus {
    /// No authentication needed or configured.
    None,
    /// Bearer token from environment variable.
    BearerToken(String),
    /// OAuth authenticated with stored tokens (supports auto-refresh).
    OAuth(OAuthState),
    /// Authentication required but not yet performed.
    NeedsLogin {
        server_name: String,
        base_url: String,
    },
}

/// Active OAuth session state with token details.
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client_id: String,
}

// ---------------------------------------------------------------------------
// Auth Provider
// ---------------------------------------------------------------------------

/// Unified authentication provider for MCP HTTP connections.
///
/// Resolves the appropriate authentication method based on configuration:
/// 1. If `bearer_token_env_var` is set, uses the env var value as Bearer token.
/// 2. If stored OAuth tokens exist and are valid, uses them (with auto-refresh).
/// 3. If OAuth metadata is discoverable but no tokens stored, signals login needed.
/// 4. Otherwise, no authentication.
#[allow(dead_code)]
pub struct AuthProvider {
    status: RwLock<AuthStatus>,
    #[allow(dead_code)]
    data_dir: PathBuf,
    #[allow(dead_code)]
    http_client: reqwest::Client,
}

impl AuthProvider {
    /// Create an AuthProvider from MCP server config parameters.
    ///
    /// This resolves the initial auth status by checking:
    /// - Environment variable for bearer token
    /// - Stored OAuth tokens on disk
    /// - OAuth discovery to determine if login is possible
    pub async fn from_config(
        server_name: &str,
        url: Option<&str>,
        bearer_token_env_var: Option<&str>,
        _scopes: Option<&[String]>,
        _oauth_resource: Option<&str>,
        data_dir: &Path,
    ) -> anyhow::Result<Self> {
        let http_client = bifrost_core::outbound_reqwest_client_builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

        // Check bearer token env var first
        if let Some(env_var) = bearer_token_env_var {
            if let Ok(token) = std::env::var(env_var) {
                if !token.is_empty() {
                    debug!(
                        server = %server_name,
                        env_var = %env_var,
                        "using bearer token from environment variable"
                    );
                    return Ok(Self {
                        status: RwLock::new(AuthStatus::BearerToken(token)),
                        data_dir: data_dir.to_path_buf(),
                        http_client,
                    });
                }
            }
        }

        let Some(base_url) = url else {
            return Ok(Self {
                status: RwLock::new(AuthStatus::None),
                data_dir: data_dir.to_path_buf(),
                http_client,
            });
        };

        // Check for stored OAuth tokens
        let store = OAuthTokenStore::new(data_dir);
        if let Ok(Some(tokens)) = store.load(server_name) {
            if !OAuthTokenStore::needs_refresh(&tokens) {
                debug!(server = %server_name, "found valid stored OAuth tokens");
                // We need token_endpoint for future refreshes — attempt discovery
                let token_endpoint = match discover_oauth_metadata(base_url).await {
                    Ok(Some(meta)) => meta.token_endpoint,
                    _ => base_url.to_string(),
                };
                return Ok(Self {
                    status: RwLock::new(AuthStatus::OAuth(OAuthState {
                        access_token: tokens.access_token,
                        refresh_token: tokens.refresh_token,
                        expires_at: tokens.expires_at,
                        token_endpoint,
                        client_id: tokens.client_id,
                    })),
                    data_dir: data_dir.to_path_buf(),
                    http_client,
                });
            } else if let Some(ref refresh_token) = tokens.refresh_token {
                // Token expired but we have a refresh token — try refreshing
                debug!(server = %server_name, "stored OAuth token expired, attempting refresh");
                let token_endpoint = match discover_oauth_metadata(base_url).await {
                    Ok(Some(meta)) => meta.token_endpoint,
                    _ => {
                        warn!(server = %server_name, "cannot refresh: OAuth discovery failed");
                        return Ok(Self {
                            status: RwLock::new(AuthStatus::NeedsLogin {
                                server_name: server_name.to_string(),
                                base_url: base_url.to_string(),
                            }),
                            data_dir: data_dir.to_path_buf(),
                            http_client,
                        });
                    }
                };

                let refresher = OAuthTokenRefresher::new(&token_endpoint, &tokens.client_id);
                match refresher.refresh(refresh_token).await {
                    Ok(new_tokens) => {
                        let now = current_unix_secs();
                        let expires_at = new_tokens.expires_in.map(|ei| now + ei);
                        let stored = StoredOAuthTokens {
                            server_name: server_name.to_string(),
                            url: base_url.to_string(),
                            client_id: tokens.client_id.clone(),
                            access_token: new_tokens.access_token.clone(),
                            refresh_token: new_tokens
                                .refresh_token
                                .clone()
                                .or(Some(refresh_token.to_string())),
                            token_type: new_tokens.token_type.clone(),
                            expires_at,
                            scope: new_tokens.scope.clone(),
                        };
                        let _ = store.save(&stored);
                        return Ok(Self {
                            status: RwLock::new(AuthStatus::OAuth(OAuthState {
                                access_token: new_tokens.access_token,
                                refresh_token: new_tokens
                                    .refresh_token
                                    .or(Some(refresh_token.to_string())),
                                expires_at,
                                token_endpoint,
                                client_id: tokens.client_id,
                            })),
                            data_dir: data_dir.to_path_buf(),
                            http_client,
                        });
                    }
                    Err(e) => {
                        warn!(server = %server_name, error = %e, "token refresh failed");
                    }
                }
            }
            // Fall through to NeedsLogin
        }

        // Check if OAuth is discoverable
        match discover_oauth_metadata(base_url).await {
            Ok(Some(_)) => Ok(Self {
                status: RwLock::new(AuthStatus::NeedsLogin {
                    server_name: server_name.to_string(),
                    base_url: base_url.to_string(),
                }),
                data_dir: data_dir.to_path_buf(),
                http_client,
            }),
            _ => Ok(Self {
                status: RwLock::new(AuthStatus::None),
                data_dir: data_dir.to_path_buf(),
                http_client,
            }),
        }
    }

    /// Get the current access token, refreshing if needed.
    ///
    /// Returns `None` if no authentication is configured or login is required.
    pub async fn get_token(&self) -> anyhow::Result<Option<String>> {
        let status = self.status.read().await.clone();
        match status {
            AuthStatus::None => Ok(None),
            AuthStatus::BearerToken(token) => Ok(Some(token)),
            AuthStatus::OAuth(ref state) => {
                // Check if refresh is needed
                if let Some(expires_at) = state.expires_at {
                    let now = current_unix_secs();
                    if now.saturating_add(REFRESH_SKEW_SECS) >= expires_at {
                        // Need to refresh
                        if let Some(ref refresh_token) = state.refresh_token {
                            return self.refresh_and_update(state, refresh_token).await;
                        }
                        // No refresh token, token is expired
                        warn!("OAuth token expired and no refresh token available");
                        return Ok(None);
                    }
                }
                Ok(Some(state.access_token.clone()))
            }
            AuthStatus::NeedsLogin { .. } => Ok(None),
        }
    }

    /// Check if login is required.
    pub async fn needs_login(&self) -> bool {
        matches!(*self.status.read().await, AuthStatus::NeedsLogin { .. })
    }

    /// Perform the OAuth login flow.
    pub async fn perform_login(&self, scopes: &[String]) -> anyhow::Result<()> {
        let (server_name, base_url) = {
            let status = self.status.read().await;
            match &*status {
                AuthStatus::NeedsLogin {
                    server_name,
                    base_url,
                } => (server_name.clone(), base_url.clone()),
                _ => {
                    return Err(anyhow::anyhow!(
                        "login not required: current auth status does not indicate login needed"
                    ));
                }
            }
        };

        let flow = OAuthLoginFlow::new(
            &server_name,
            &base_url,
            scopes.to_vec(),
            None,
            &self.data_dir,
        );

        let stored = flow.execute().await?;

        // Discover token endpoint for future refreshes
        let token_endpoint = match discover_oauth_metadata(&base_url).await {
            Ok(Some(meta)) => meta.token_endpoint,
            _ => base_url.clone(),
        };

        // Update status
        let mut status = self.status.write().await;
        *status = AuthStatus::OAuth(OAuthState {
            access_token: stored.access_token,
            refresh_token: stored.refresh_token,
            expires_at: stored.expires_at,
            token_endpoint,
            client_id: stored.client_id,
        });

        Ok(())
    }

    /// Refresh the token and update internal state.
    async fn refresh_and_update(
        &self,
        state: &OAuthState,
        refresh_token: &str,
    ) -> anyhow::Result<Option<String>> {
        let refresher = OAuthTokenRefresher::new(&state.token_endpoint, &state.client_id);
        match refresher.refresh(refresh_token).await {
            Ok(new_tokens) => {
                let now = current_unix_secs();
                let expires_at = new_tokens.expires_in.map(|ei| now + ei);
                let new_access_token = new_tokens.access_token.clone();

                // Update status
                let mut status = self.status.write().await;
                *status = AuthStatus::OAuth(OAuthState {
                    access_token: new_tokens.access_token,
                    refresh_token: new_tokens
                        .refresh_token
                        .or_else(|| Some(refresh_token.to_string())),
                    expires_at,
                    token_endpoint: state.token_endpoint.clone(),
                    client_id: state.client_id.clone(),
                });

                Ok(Some(new_access_token))
            }
            Err(e) => {
                error!(error = %e, "failed to refresh OAuth token");
                Err(e)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// OAuth Persistor (auto-persist & refresh pattern)
// ---------------------------------------------------------------------------

/// Automatic OAuth token persistence manager.
///
/// Wraps token storage and refresh logic to automatically:
/// - Persist tokens after each API operation (if changed).
/// - Refresh tokens before each API operation (if near expiry).
///
/// This mirrors Codex's `OAuthPersistor` pattern where every tool call
/// is bracketed by `refresh_if_needed()` → operation → `persist_if_needed()`.
#[derive(Clone)]
pub struct OAuthPersistor {
    inner: Arc<OAuthPersistorInner>,
}

struct OAuthPersistorInner {
    server_name: String,
    #[allow(dead_code)]
    url: String,
    store: OAuthTokenStore,
    refresher: Option<OAuthTokenRefresher>,
    last_access_token: RwLock<Option<String>>,
}

impl OAuthPersistor {
    /// Create a new OAuthPersistor.
    ///
    /// - `server_name`: Name of the MCP server (used as storage key).
    /// - `url`: Server URL.
    /// - `data_dir`: Data directory for token storage.
    /// - `token_endpoint`: OAuth token endpoint for refresh requests.
    /// - `client_id`: OAuth client ID.
    /// - `initial_access_token`: Current access token (for change detection).
    pub fn new(
        server_name: &str,
        url: &str,
        data_dir: &Path,
        token_endpoint: Option<&str>,
        client_id: Option<&str>,
        initial_access_token: Option<&str>,
    ) -> Self {
        let refresher = match (token_endpoint, client_id) {
            (Some(te), Some(cid)) => Some(OAuthTokenRefresher::new(te, cid)),
            _ => None,
        };

        Self {
            inner: Arc::new(OAuthPersistorInner {
                server_name: server_name.to_string(),
                url: url.to_string(),
                store: OAuthTokenStore::new(data_dir),
                refresher,
                last_access_token: RwLock::new(initial_access_token.map(|s| s.to_string())),
            }),
        }
    }

    /// Persist tokens if they have changed since last persist.
    ///
    /// Call this after each API operation to ensure token updates
    /// (e.g., from server-side refresh) are saved to disk.
    pub async fn persist_if_needed(
        &self,
        current_tokens: &StoredOAuthTokens,
    ) -> anyhow::Result<()> {
        let last = self.inner.last_access_token.read().await;
        if last.as_deref() == Some(&current_tokens.access_token) {
            return Ok(());
        }
        drop(last);

        // Token changed, persist it
        self.inner.store.save(current_tokens)?;
        let mut last = self.inner.last_access_token.write().await;
        *last = Some(current_tokens.access_token.clone());

        debug!(
            server = %self.inner.server_name,
            "persisted updated OAuth tokens"
        );
        Ok(())
    }

    /// Refresh tokens if they are near expiry.
    ///
    /// Call this before each API operation to ensure a valid access token.
    /// Returns the new token response if a refresh was performed.
    pub async fn refresh_if_needed(
        &self,
        current_tokens: &StoredOAuthTokens,
    ) -> anyhow::Result<Option<StoredOAuthTokens>> {
        if !OAuthTokenStore::needs_refresh(current_tokens) {
            return Ok(None);
        }

        let refresher = self.inner.refresher.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "token refresh needed for server '{}' but no refresher configured",
                self.inner.server_name
            )
        })?;

        let refresh_token = current_tokens.refresh_token.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "token refresh needed for server '{}' but no refresh_token available",
                self.inner.server_name
            )
        })?;

        info!(
            server = %self.inner.server_name,
            "refreshing OAuth token (near expiry)"
        );

        let new_tokens = refresher.refresh(refresh_token).await?;
        let now = current_unix_secs();
        let expires_at = new_tokens.expires_in.map(|ei| now + ei);

        let stored = StoredOAuthTokens {
            server_name: current_tokens.server_name.clone(),
            url: current_tokens.url.clone(),
            client_id: current_tokens.client_id.clone(),
            access_token: new_tokens.access_token,
            refresh_token: new_tokens
                .refresh_token
                .or_else(|| current_tokens.refresh_token.clone()),
            token_type: new_tokens.token_type,
            expires_at,
            scope: new_tokens.scope,
        };

        // Persist the refreshed tokens
        self.inner.store.save(&stored)?;
        let mut last = self.inner.last_access_token.write().await;
        *last = Some(stored.access_token.clone());

        Ok(Some(stored))
    }
}

impl std::fmt::Debug for OAuthPersistor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthPersistor")
            .field("server_name", &self.inner.server_name)
            .field("has_refresher", &self.inner.refresher.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Get current time as Unix seconds.
fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

/// Sanitize a server name for use as a filename.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Simple URL decoding (percent-decode).
fn urlencoding_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().and_then(hex_val);
            let lo = chars.next().and_then(hex_val);
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push((h << 4 | l) as char);
            } else {
                result.push('%');
            }
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stored_tokens_serialization_roundtrip() {
        let tokens = StoredOAuthTokens {
            server_name: "test-server".to_string(),
            url: "https://example.com/mcp".to_string(),
            client_id: "client-123".to_string(),
            access_token: "access-abc".to_string(),
            refresh_token: Some("refresh-xyz".to_string()),
            token_type: Some("Bearer".to_string()),
            expires_at: Some(1700000000),
            scope: Some("read write".to_string()),
        };

        let json = serde_json::to_string(&tokens).unwrap();
        let deserialized: StoredOAuthTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(tokens, deserialized);
    }

    #[test]
    fn test_stored_tokens_minimal_deserialization() {
        let json = r#"{
            "server_name": "srv",
            "url": "https://example.com",
            "client_id": "cid",
            "access_token": "tok",
            "refresh_token": null,
            "token_type": null,
            "expires_at": null,
            "scope": null
        }"#;
        let tokens: StoredOAuthTokens = serde_json::from_str(json).unwrap();
        assert_eq!(tokens.server_name, "srv");
        assert_eq!(tokens.access_token, "tok");
        assert!(tokens.refresh_token.is_none());
        assert!(tokens.expires_at.is_none());
    }

    #[test]
    fn test_token_response_deserialization() {
        let json = r#"{
            "access_token": "new-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "new-refresh",
            "scope": "openid profile"
        }"#;
        let resp: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "new-token");
        assert_eq!(resp.token_type.as_deref(), Some("Bearer"));
        assert_eq!(resp.expires_in, Some(3600));
        assert_eq!(resp.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(resp.scope.as_deref(), Some("openid profile"));
    }

    #[test]
    fn test_token_response_minimal() {
        let json = r#"{"access_token": "tok"}"#;
        let resp: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "tok");
        assert!(resp.token_type.is_none());
        assert!(resp.expires_in.is_none());
        assert!(resp.refresh_token.is_none());
    }

    #[test]
    fn test_needs_refresh_no_expiry() {
        let tokens = StoredOAuthTokens {
            server_name: "s".to_string(),
            url: "u".to_string(),
            client_id: "c".to_string(),
            access_token: "t".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: None,
            scope: None,
        };
        // No expiry → should NOT need refresh
        assert!(!OAuthTokenStore::needs_refresh(&tokens));
    }

    #[test]
    fn test_needs_refresh_future_expiry() {
        let future = current_unix_secs() + 3600;
        let tokens = StoredOAuthTokens {
            server_name: "s".to_string(),
            url: "u".to_string(),
            client_id: "c".to_string(),
            access_token: "t".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: Some(future),
            scope: None,
        };
        // 1 hour from now → should NOT need refresh
        assert!(!OAuthTokenStore::needs_refresh(&tokens));
    }

    #[test]
    fn test_needs_refresh_expired() {
        let past = current_unix_secs().saturating_sub(100);
        let tokens = StoredOAuthTokens {
            server_name: "s".to_string(),
            url: "u".to_string(),
            client_id: "c".to_string(),
            access_token: "t".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: Some(past),
            scope: None,
        };
        // Already expired → should need refresh
        assert!(OAuthTokenStore::needs_refresh(&tokens));
    }

    #[test]
    fn test_needs_refresh_within_skew() {
        let almost_expired = current_unix_secs() + REFRESH_SKEW_SECS - 1;
        let tokens = StoredOAuthTokens {
            server_name: "s".to_string(),
            url: "u".to_string(),
            client_id: "c".to_string(),
            access_token: "t".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: Some(almost_expired),
            scope: None,
        };
        // Within skew window → should need refresh
        assert!(OAuthTokenStore::needs_refresh(&tokens));
    }

    #[test]
    fn test_pkce_challenge_generation() {
        let pkce = generate_pkce_challenge();
        // code_verifier should be 43 chars (32 bytes base64url-encoded without padding)
        assert_eq!(pkce.code_verifier.len(), 43);
        // code_challenge should be 43 chars (32 bytes SHA-256 hash base64url-encoded)
        assert_eq!(pkce.code_challenge.len(), 43);
        // They should be different
        assert_ne!(pkce.code_verifier, pkce.code_challenge);
        // Verify the challenge matches the verifier
        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let hash = hasher.finalize();
        let expected_challenge = URL_SAFE_NO_PAD.encode(hash);
        assert_eq!(pkce.code_challenge, expected_challenge);
    }

    #[test]
    fn test_pkce_uniqueness() {
        let pkce1 = generate_pkce_challenge();
        let pkce2 = generate_pkce_challenge();
        // Two consecutive generations should produce different values
        assert_ne!(pkce1.code_verifier, pkce2.code_verifier);
        assert_ne!(pkce1.code_challenge, pkce2.code_challenge);
    }

    #[test]
    fn test_generate_state() {
        let state1 = generate_state();
        let state2 = generate_state();
        // Should be 22 chars (16 bytes base64url-encoded without padding)
        assert_eq!(state1.len(), 22);
        // Should be unique
        assert_ne!(state1, state2);
    }

    #[test]
    fn test_build_authorization_url() {
        let url = build_authorization_url(
            "https://auth.example.com/authorize",
            "client-123",
            "http://127.0.0.1:8080/callback",
            &["read".to_string(), "write".to_string()],
            "state-abc",
            "challenge-xyz",
            Some("https://api.example.com"),
        )
        .unwrap();

        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-123"));
        assert!(url.contains("state=state-abc"));
        assert!(url.contains("code_challenge=challenge-xyz"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=read+write"));
        assert!(url.contains("resource="));
    }

    #[test]
    fn test_build_authorization_url_no_scopes_no_resource() {
        let url = build_authorization_url(
            "https://auth.example.com/authorize",
            "cid",
            "http://127.0.0.1:9999/cb",
            &[],
            "st",
            "ch",
            None,
        )
        .unwrap();

        assert!(url.contains("response_type=code"));
        assert!(!url.contains("scope="));
        assert!(!url.contains("resource="));
    }

    #[test]
    fn test_discovery_paths_root() {
        let paths = discovery_paths("/");
        assert_eq!(paths, vec!["/.well-known/oauth-authorization-server"]);
    }

    #[test]
    fn test_discovery_paths_with_path() {
        let paths = discovery_paths("/mcp/v1");
        assert_eq!(
            paths,
            vec![
                "/.well-known/oauth-authorization-server/mcp/v1",
                "/mcp/v1/.well-known/oauth-authorization-server",
                "/.well-known/oauth-authorization-server",
            ]
        );
    }

    #[test]
    fn test_discovery_paths_empty() {
        let paths = discovery_paths("");
        assert_eq!(paths, vec!["/.well-known/oauth-authorization-server"]);
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("simple"), "simple");
        assert_eq!(sanitize_filename("with-dash"), "with-dash");
        assert_eq!(sanitize_filename("with_under"), "with_under");
        assert_eq!(sanitize_filename("with spaces"), "with_spaces");
        assert_eq!(
            sanitize_filename("special/chars.here"),
            "special_chars_here"
        );
        assert_eq!(
            sanitize_filename("https://example.com"),
            "https___example_com"
        );
    }

    #[test]
    fn test_urlencoding_decode() {
        assert_eq!(urlencoding_decode("hello%20world"), "hello world");
        assert_eq!(urlencoding_decode("a+b"), "a b");
        assert_eq!(urlencoding_decode("no%2Fslash"), "no/slash");
        assert_eq!(urlencoding_decode("plain"), "plain");
        assert_eq!(urlencoding_decode("%3A%2F%2F"), "://");
    }

    #[test]
    fn test_token_store_save_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = OAuthTokenStore::new(dir.path());

        let tokens = StoredOAuthTokens {
            server_name: "test-server".to_string(),
            url: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            access_token: "access-123".to_string(),
            refresh_token: Some("refresh-456".to_string()),
            token_type: Some("Bearer".to_string()),
            expires_at: Some(9999999999),
            scope: Some("read".to_string()),
        };

        // Save
        store.save(&tokens).unwrap();

        // Load
        let loaded = store.load("test-server").unwrap().unwrap();
        assert_eq!(loaded, tokens);

        // Delete
        let deleted = store.delete("test-server").unwrap();
        assert!(deleted);

        // Load after delete
        let loaded_after = store.load("test-server").unwrap();
        assert!(loaded_after.is_none());

        // Delete non-existent
        let deleted_again = store.delete("test-server").unwrap();
        assert!(!deleted_again);
    }

    #[test]
    fn test_oauth_server_metadata_deserialization() {
        let json = r#"{
            "issuer": "https://auth.example.com",
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token",
            "scopes_supported": ["openid", "profile", "email"],
            "registration_endpoint": "https://auth.example.com/register",
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"]
        }"#;
        let metadata: OAuthServerMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.issuer.as_deref(), Some("https://auth.example.com"));
        assert_eq!(
            metadata.authorization_endpoint,
            "https://auth.example.com/authorize"
        );
        assert_eq!(metadata.token_endpoint, "https://auth.example.com/token");
        assert_eq!(metadata.scopes_supported.len(), 3);
        assert!(metadata.registration_endpoint.is_some());
        assert_eq!(
            metadata.code_challenge_methods_supported,
            Some(vec!["S256".to_string()])
        );
    }

    #[test]
    fn test_oauth_server_metadata_minimal() {
        let json = r#"{
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token"
        }"#;
        let metadata: OAuthServerMetadata = serde_json::from_str(json).unwrap();
        assert!(metadata.issuer.is_none());
        assert!(metadata.scopes_supported.is_empty());
        assert!(metadata.registration_endpoint.is_none());
        assert!(metadata.code_challenge_methods_supported.is_none());
    }

    #[test]
    fn test_current_unix_secs_reasonable() {
        let now = current_unix_secs();
        // Should be after 2020-01-01 (1577836800)
        assert!(now > 1_577_836_800);
    }

    #[test]
    fn test_hex_val() {
        assert_eq!(hex_val(b'0'), Some(0));
        assert_eq!(hex_val(b'9'), Some(9));
        assert_eq!(hex_val(b'a'), Some(10));
        assert_eq!(hex_val(b'f'), Some(15));
        assert_eq!(hex_val(b'A'), Some(10));
        assert_eq!(hex_val(b'F'), Some(15));
        assert_eq!(hex_val(b'g'), None);
        assert_eq!(hex_val(b' '), None);
    }

    #[test]
    fn test_oauth_persistor_creation() {
        let dir = tempfile::tempdir().unwrap();
        let persistor = OAuthPersistor::new(
            "test-server",
            "https://example.com",
            dir.path(),
            Some("https://auth.example.com/token"),
            Some("client-id"),
            Some("initial-token"),
        );
        assert_eq!(
            format!("{:?}", persistor),
            "OAuthPersistor { server_name: \"test-server\", has_refresher: true }"
        );
    }

    #[test]
    fn test_oauth_persistor_without_refresher() {
        let dir = tempfile::tempdir().unwrap();
        let persistor = OAuthPersistor::new(
            "test-server",
            "https://example.com",
            dir.path(),
            None,
            None,
            None,
        );
        assert_eq!(
            format!("{:?}", persistor),
            "OAuthPersistor { server_name: \"test-server\", has_refresher: false }"
        );
    }

    #[tokio::test]
    async fn test_oauth_persistor_persist_if_needed_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let persistor = OAuthPersistor::new(
            "test-server",
            "https://example.com",
            dir.path(),
            None,
            None,
            Some("access-123"),
        );

        let tokens = StoredOAuthTokens {
            server_name: "test-server".to_string(),
            url: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            access_token: "access-123".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: None,
            scope: None,
        };

        // Same token — should not persist
        persistor.persist_if_needed(&tokens).await.unwrap();

        // Verify nothing was saved (no file)
        let store = OAuthTokenStore::new(dir.path());
        assert!(store.load("test-server").unwrap().is_none());
    }

    #[tokio::test]
    async fn test_oauth_persistor_persist_if_needed_changed() {
        let dir = tempfile::tempdir().unwrap();
        let persistor = OAuthPersistor::new(
            "test-server",
            "https://example.com",
            dir.path(),
            None,
            None,
            Some("old-token"),
        );

        let tokens = StoredOAuthTokens {
            server_name: "test-server".to_string(),
            url: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            access_token: "new-token".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: None,
            scope: None,
        };

        // Different token — should persist
        persistor.persist_if_needed(&tokens).await.unwrap();

        // Verify it was saved
        let store = OAuthTokenStore::new(dir.path());
        let loaded = store.load("test-server").unwrap().unwrap();
        assert_eq!(loaded.access_token, "new-token");
    }

    #[tokio::test]
    async fn test_oauth_persistor_refresh_not_needed() {
        let dir = tempfile::tempdir().unwrap();
        let persistor = OAuthPersistor::new(
            "test-server",
            "https://example.com",
            dir.path(),
            Some("https://auth.example.com/token"),
            Some("cid"),
            None,
        );

        let far_future = current_unix_secs() + 3600;
        let tokens = StoredOAuthTokens {
            server_name: "test-server".to_string(),
            url: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            access_token: "tok".to_string(),
            refresh_token: Some("refresh".to_string()),
            token_type: None,
            expires_at: Some(far_future),
            scope: None,
        };

        // Token is fresh — refresh_if_needed should return None
        let result = persistor.refresh_if_needed(&tokens).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_credentials_store_mode_default() {
        assert_eq!(
            OAuthCredentialsStoreMode::default(),
            OAuthCredentialsStoreMode::Auto
        );
    }

    #[test]
    fn test_save_load_with_file_mode() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = StoredOAuthTokens {
            server_name: "mode-test".to_string(),
            url: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            access_token: "tok-mode".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: None,
            scope: None,
        };

        save_oauth_tokens_with_mode(&tokens, dir.path(), OAuthCredentialsStoreMode::File).unwrap();
        let loaded = load_oauth_tokens_with_mode(
            "mode-test",
            "https://example.com",
            dir.path(),
            OAuthCredentialsStoreMode::File,
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.access_token, "tok-mode");
    }

    #[test]
    fn test_save_load_with_auto_mode_fallback_to_file() {
        // Without keyring-store feature, Auto should fall back to file
        let dir = tempfile::tempdir().unwrap();
        let tokens = StoredOAuthTokens {
            server_name: "auto-test".to_string(),
            url: "https://example.com".to_string(),
            client_id: "cid".to_string(),
            access_token: "tok-auto".to_string(),
            refresh_token: None,
            token_type: None,
            expires_at: None,
            scope: None,
        };

        save_oauth_tokens_with_mode(&tokens, dir.path(), OAuthCredentialsStoreMode::Auto).unwrap();
        let loaded = load_oauth_tokens_with_mode(
            "auto-test",
            "https://example.com",
            dir.path(),
            OAuthCredentialsStoreMode::Auto,
        )
        .unwrap()
        .unwrap();
        assert_eq!(loaded.access_token, "tok-auto");
    }
}
