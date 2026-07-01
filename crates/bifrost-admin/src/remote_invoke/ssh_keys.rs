use base64::Engine;
use parking_lot::Mutex;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use bifrost_core::{BifrostError, Result};

use super::types::{CallerInfo, GrantMode, SshDeviceRoute};

const SSH_KEYS_DB_FILE: &str = "remote_invoke_ssh_keys.db";
const SSH_KEYS_ENCRYPTION_KEY_FILE: &str = "remote_invoke_ssh_keys.key";
const BIFROST_KEY_BEGIN: &str = "-----BEGIN BIFROST KEY-----";
const BIFROST_KEY_END: &str = "-----END BIFROST KEY-----";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshKeyStatus {
    Active,
    Revoked,
}

impl SshKeyStatus {
    fn from_db(value: &str) -> Self {
        match value {
            "revoked" => Self::Revoked,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKeyRecord {
    pub id: String,
    pub device_code: String,
    pub label: String,
    pub public_key_pem: String,
    pub ssh_key_fingerprint: String,
    pub grant_mode: GrantMode,
    pub status: SshKeyStatus,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_caller_info: Option<CallerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshKeyMaterial {
    #[serde(flatten)]
    pub record: SshKeyRecord,
    pub bifrost_key_file: String,
}

#[derive(Debug, Clone)]
pub struct ActiveSshKey {
    pub record: SshKeyRecord,
    pub decrypted_private_key_pkcs8: Vec<u8>,
}

pub struct SshKeyStore {
    db_path: PathBuf,
    encryption: EncryptionKeyProvider,
    conn: Mutex<Option<Connection>>,
    init_error: Option<String>,
}

impl SshKeyStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        let db_path = admin_dir.join(SSH_KEYS_DB_FILE);
        let encryption = EncryptionKeyProvider::new(admin_dir.join(SSH_KEYS_ENCRYPTION_KEY_FILE));

        let init = || -> std::result::Result<Connection, String> {
            std::fs::create_dir_all(&admin_dir).map_err(|e| format!("create admin dir: {e}"))?;
            harden_private_dir(&admin_dir).map_err(|e| format!("harden admin dir: {e}"))?;
            let conn = Connection::open(&db_path).map_err(|e| format!("open ssh key db: {e}"))?;
            harden_private_file(&db_path).map_err(|e| format!("harden ssh key db: {e}"))?;
            initialize_schema(&conn).map_err(|e| format!("init ssh key schema: {e}"))?;
            Ok(conn)
        };

        match init() {
            Ok(conn) => Self {
                db_path,
                encryption,
                conn: Mutex::new(Some(conn)),
                init_error: None,
            },
            Err(error) => Self {
                db_path,
                encryption,
                conn: Mutex::new(None),
                init_error: Some(error),
            },
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn active_route(&self) -> Result<Option<SshDeviceRoute>> {
        let active = self.get_active_key()?;
        Ok(active.map(|key| SshDeviceRoute {
            device_code: key.record.device_code,
            public_key_pem: key.record.public_key_pem,
        }))
    }

    pub fn get_active_key(&self) -> Result<Option<ActiveSshKey>> {
        let guard = self.conn.lock();
        let conn = self.require_conn(&guard)?;
        let row = load_active_row(conn)?;
        match row {
            Some(row) => Ok(Some(self.inflate_active_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn create_or_replace_key(
        &self,
        label: String,
        grant_mode: GrantMode,
    ) -> Result<SshKeyMaterial> {
        let grant_mode = normalize_ssh_grant_mode(grant_mode);
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| BifrostError::Config("generate ed25519 keypair failed".to_string()))?;
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| {
            BifrostError::Config("parse generated ed25519 keypair failed".to_string())
        })?;
        let public_key_der = ed25519_public_key_to_spki_der(key_pair.public_key().as_ref());
        let public_key_pem = spki_der_to_pem(&public_key_der);
        let ssh_key_fingerprint = sha256_hex(&public_key_der);
        let device_code = derive_device_code(&public_key_der);
        let encrypted_private_key = self.encryption.encrypt(pkcs8.as_ref())?;
        let now = now_millis();
        let id = uuid::Uuid::new_v4().to_string();

        {
            let mut guard = self.conn.lock();
            let conn = self.require_conn_mut(&mut guard)?;
            let tx = conn
                .transaction()
                .map_err(|e| BifrostError::Config(format!("start ssh key transaction: {e}")))?;
            tx.execute(
                "UPDATE remote_invoke_ssh_keys SET status = 'revoked' WHERE status = 'active'",
                [],
            )
            .map_err(|e| BifrostError::Config(format!("revoke active ssh keys: {e}")))?;
            tx.execute(
                "INSERT INTO remote_invoke_ssh_keys (
                    id, device_code, label, public_key_pem, ssh_key_fingerprint,
                    private_key_pem_encrypted, grant_mode, status, created_at, last_used_at, last_caller_info_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active', ?8, NULL, NULL)",
                params![
                    id,
                    device_code,
                    label,
                    public_key_pem,
                    ssh_key_fingerprint,
                    encrypted_private_key,
                    serde_json::to_string(&grant_mode).map_err(|e| BifrostError::Config(format!("serialize grant_mode: {e}")))?,
                    now,
                ],
            )
            .map_err(|e| BifrostError::Config(format!("insert ssh key: {e}")))?;
            tx.commit()
                .map_err(|e| BifrostError::Config(format!("commit ssh key transaction: {e}")))?;
        }

        let record = SshKeyRecord {
            id,
            device_code: device_code.clone(),
            label,
            public_key_pem,
            ssh_key_fingerprint,
            grant_mode,
            status: SshKeyStatus::Active,
            created_at: now,
            last_used_at: None,
            last_caller_info: None,
        };
        let bifrost_key_file = render_bifrost_key_file(&device_code, pkcs8.as_ref());
        Ok(SshKeyMaterial {
            record,
            bifrost_key_file,
        })
    }

    pub fn update_active_key(
        &self,
        label: Option<String>,
        grant_mode: Option<GrantMode>,
    ) -> Result<Option<SshKeyRecord>> {
        let mut guard = self.conn.lock();
        let conn = self.require_conn_mut(&mut guard)?;
        let Some(existing) = load_active_row(conn)? else {
            return Ok(None);
        };

        let next_label = label.unwrap_or_else(|| existing.label.clone());
        let next_grant_mode = normalize_ssh_grant_mode(grant_mode.unwrap_or(existing.grant_mode));
        conn.execute(
            "UPDATE remote_invoke_ssh_keys SET label = ?1, grant_mode = ?2 WHERE id = ?3",
            params![
                next_label,
                serde_json::to_string(&next_grant_mode)
                    .map_err(|e| BifrostError::Config(format!("serialize grant_mode: {e}")))?,
                existing.id,
            ],
        )
        .map_err(|e| BifrostError::Config(format!("update active ssh key: {e}")))?;

        Ok(Some(SshKeyRecord {
            label: next_label,
            grant_mode: next_grant_mode,
            ..existing.into_record()
        }))
    }

    pub fn revoke_active_key(&self) -> Result<Option<SshKeyRecord>> {
        let mut guard = self.conn.lock();
        let conn = self.require_conn_mut(&mut guard)?;
        let Some(existing) = load_active_row(conn)? else {
            return Ok(None);
        };
        conn.execute(
            "UPDATE remote_invoke_ssh_keys SET status = 'revoked' WHERE id = ?1",
            params![existing.id],
        )
        .map_err(|e| BifrostError::Config(format!("revoke active ssh key: {e}")))?;
        let mut record = existing.into_record();
        record.status = SshKeyStatus::Revoked;
        Ok(Some(record))
    }

    pub fn export_active_key_file(&self) -> Result<Option<String>> {
        let active = self.get_active_key()?;
        Ok(active.map(|key| {
            render_bifrost_key_file(&key.record.device_code, &key.decrypted_private_key_pkcs8)
        }))
    }

    pub fn export_active_key_material(&self) -> Result<Option<SshKeyMaterial>> {
        let active = self.get_active_key()?;
        Ok(active.map(|key| SshKeyMaterial {
            bifrost_key_file: render_bifrost_key_file(
                &key.record.device_code,
                &key.decrypted_private_key_pkcs8,
            ),
            record: key.record,
        }))
    }

    pub fn mark_used(&self, device_code: &str, caller_info: Option<&CallerInfo>) -> Result<()> {
        let caller_info_json = caller_info
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| BifrostError::Config(format!("serialize caller_info: {e}")))?;
        let mut guard = self.conn.lock();
        let conn = self.require_conn_mut(&mut guard)?;
        conn.execute(
            "UPDATE remote_invoke_ssh_keys
             SET last_used_at = ?1, last_caller_info_json = ?2
             WHERE device_code = ?3 AND status = 'active'",
            params![now_millis(), caller_info_json, device_code],
        )
        .map_err(|e| BifrostError::Config(format!("update ssh key last_used_at: {e}")))?;
        Ok(())
    }

    fn inflate_active_row(&self, row: StoredSshKeyRow) -> Result<ActiveSshKey> {
        let decrypted_private_key_pkcs8 =
            self.encryption.decrypt(&row.private_key_pem_encrypted)?;
        Ok(ActiveSshKey {
            record: row.into_record(),
            decrypted_private_key_pkcs8,
        })
    }

    fn require_conn<'a>(&self, conn: &'a Option<Connection>) -> Result<&'a Connection> {
        conn.as_ref().ok_or_else(|| {
            BifrostError::Config(
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "ssh key store unavailable".to_string()),
            )
        })
    }

    fn require_conn_mut<'a>(&self, conn: &'a mut Option<Connection>) -> Result<&'a mut Connection> {
        conn.as_mut().ok_or_else(|| {
            BifrostError::Config(
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "ssh key store unavailable".to_string()),
            )
        })
    }
}

#[derive(Debug, Clone)]
struct StoredSshKeyRow {
    id: String,
    device_code: String,
    label: String,
    public_key_pem: String,
    ssh_key_fingerprint: String,
    private_key_pem_encrypted: String,
    grant_mode: GrantMode,
    status: SshKeyStatus,
    created_at: u64,
    last_used_at: Option<u64>,
    last_caller_info: Option<CallerInfo>,
}

impl StoredSshKeyRow {
    fn into_record(self) -> SshKeyRecord {
        SshKeyRecord {
            id: self.id,
            device_code: self.device_code,
            label: self.label,
            public_key_pem: self.public_key_pem,
            ssh_key_fingerprint: self.ssh_key_fingerprint,
            grant_mode: normalize_ssh_grant_mode(self.grant_mode),
            status: self.status,
            created_at: self.created_at,
            last_used_at: self.last_used_at,
            last_caller_info: self.last_caller_info,
        }
    }
}

fn normalize_ssh_grant_mode(mode: GrantMode) -> GrantMode {
    mode
}

fn initialize_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS remote_invoke_ssh_keys (
            id TEXT PRIMARY KEY,
            device_code TEXT NOT NULL UNIQUE,
            label TEXT NOT NULL,
            public_key_pem TEXT NOT NULL,
            ssh_key_fingerprint TEXT NOT NULL UNIQUE,
            private_key_pem_encrypted TEXT NOT NULL,
            grant_mode TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER,
            last_caller_info_json TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_remote_invoke_ssh_keys_status
            ON remote_invoke_ssh_keys(status);",
    )
}

fn load_active_row(conn: &Connection) -> Result<Option<StoredSshKeyRow>> {
    conn.query_row(
        "SELECT
            id, device_code, label, public_key_pem, ssh_key_fingerprint,
            private_key_pem_encrypted, grant_mode, status, created_at,
            last_used_at, last_caller_info_json
         FROM remote_invoke_ssh_keys
         WHERE status = 'active'
         ORDER BY created_at DESC
         LIMIT 1",
        [],
        |row| {
            let grant_mode_raw: String = row.get(6)?;
            let grant_mode = serde_json::from_str(&grant_mode_raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let last_caller_info_json: Option<String> = row.get(10)?;
            let last_caller_info = last_caller_info_json
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        10,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

            Ok(StoredSshKeyRow {
                id: row.get(0)?,
                device_code: row.get(1)?,
                label: row.get(2)?,
                public_key_pem: row.get(3)?,
                ssh_key_fingerprint: row.get(4)?,
                private_key_pem_encrypted: row.get(5)?,
                grant_mode: normalize_ssh_grant_mode(grant_mode),
                status: SshKeyStatus::from_db(&row.get::<_, String>(7)?),
                created_at: row.get(8)?,
                last_used_at: row.get(9)?,
                last_caller_info,
            })
        },
    )
    .optional()
    .map_err(|e| BifrostError::Config(format!("query active ssh key: {e}")))
}

struct EncryptionKeyProvider {
    key_path: PathBuf,
}

impl EncryptionKeyProvider {
    fn new(key_path: PathBuf) -> Self {
        Self { key_path }
    }

    fn load_key(&self) -> Result<[u8; 32]> {
        if self.key_path.exists() {
            let encoded = std::fs::read_to_string(&self.key_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "read {}: {e}",
                    self.key_path.display()
                )))
            })?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|e| BifrostError::Config(format!("decode ssh key encryption key: {e}")))?;
            return bytes.as_slice().try_into().map_err(|_| {
                BifrostError::Config("invalid ssh key encryption key size".to_string())
            });
        }

        let mut key = [0u8; 32];
        SystemRandom::new().fill(&mut key).map_err(|_| {
            BifrostError::Config("generate ssh key encryption key failed".to_string())
        })?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        std::fs::write(&self.key_path, encoded).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.key_path.display()
            )))
        })?;
        harden_private_file(&self.key_path)?;
        Ok(key)
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let key_bytes = self.load_key()?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            BifrostError::Config("create ssh key encryption cipher failed".to_string())
        })?;
        let key = LessSafeKey::new(unbound);

        let mut nonce_bytes = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| BifrostError::Config("generate ssh key nonce failed".to_string()))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut in_out = plaintext.to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| BifrostError::Config("encrypt ssh private key failed".to_string()))?;

        let mut encoded = Vec::with_capacity(NONCE_LEN + in_out.len());
        encoded.extend_from_slice(&nonce_bytes);
        encoded.extend_from_slice(&in_out);
        Ok(base64::engine::general_purpose::STANDARD.encode(encoded))
    }

    fn decrypt(&self, encoded: &str) -> Result<Vec<u8>> {
        let key_bytes = self.load_key()?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            BifrostError::Config("create ssh key decryption cipher failed".to_string())
        })?;
        let key = LessSafeKey::new(unbound);
        let mut combined = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| BifrostError::Config(format!("decode ssh private key envelope: {e}")))?;
        if combined.len() <= NONCE_LEN {
            return Err(BifrostError::Config(
                "invalid ssh private key envelope".to_string(),
            ));
        }
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(&combined[..NONCE_LEN]);
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let plaintext = key
            .open_in_place(nonce, Aad::empty(), &mut combined[NONCE_LEN..])
            .map_err(|_| BifrostError::Config("decrypt ssh private key failed".to_string()))?;
        Ok(plaintext.to_vec())
    }
}

#[cfg(unix)]
fn harden_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "chmod 0700 {}: {e}",
            path.display()
        )))
    })
}

#[cfg(not(unix))]
fn harden_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn harden_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "chmod 0600 {}: {e}",
            path.display()
        )))
    })
}

#[cfg(not(unix))]
fn harden_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn ed25519_public_key_to_spki_der(public_key: &[u8]) -> Vec<u8> {
    const ED25519_SPKI_PREFIX: &[u8] = &[
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];

    let mut der = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + public_key.len());
    der.extend_from_slice(ED25519_SPKI_PREFIX);
    der.extend_from_slice(public_key);
    der
}

fn spki_der_to_pem(spki_der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(spki_der);
    let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----\n");
    pem
}

fn render_bifrost_key_file(device_code: &str, private_key_pkcs8: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(private_key_pkcs8);
    format!("{BIFROST_KEY_BEGIN}\nDevice-Code: {device_code}\n{encoded}\n{BIFROST_KEY_END}\n")
}

pub fn derive_device_code(public_key_der: &[u8]) -> String {
    let digest = digest(&SHA256, public_key_der);
    let prefix = &digest.as_ref()[..8];
    let mut out = String::from("BF-");
    for byte in prefix {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = digest(&SHA256, data);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_code_is_stable_and_prefixed() {
        let code = derive_device_code(b"public-key-der");
        assert!(code.starts_with("BF-"));
        assert_eq!(code.len(), 19);
    }

    #[test]
    fn store_can_create_replace_and_export_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SshKeyStore::new(tempdir.path());

        let first = store
            .create_or_replace_key("CI".to_string(), GrantMode::Permanent)
            .expect("create first key");
        assert!(first.bifrost_key_file.contains(&first.record.device_code));
        assert_eq!(first.record.status, SshKeyStatus::Active);
        assert_eq!(first.record.grant_mode, GrantMode::Permanent);

        let active_route = store
            .active_route()
            .expect("load active route")
            .expect("active route should exist");
        assert_eq!(active_route.device_code, first.record.device_code);

        let second = store
            .create_or_replace_key("Agent".to_string(), GrantMode::OneHour)
            .expect("replace key");
        assert_ne!(first.record.device_code, second.record.device_code);
        assert_eq!(second.record.grant_mode, GrantMode::OneHour);

        let exported = store
            .export_active_key_file()
            .expect("export active key")
            .expect("file should exist");
        assert!(exported.contains(&second.record.device_code));
    }

    #[test]
    fn update_active_key_preserves_requested_ssh_grant_mode() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SshKeyStore::new(tempdir.path());

        store
            .create_or_replace_key("CI".to_string(), GrantMode::Permanent)
            .expect("create key");

        let updated = store
            .update_active_key(Some("Agent".to_string()), Some(GrantMode::ThirtyMinutes))
            .expect("update key")
            .expect("active key");

        assert_eq!(updated.label, "Agent");
        assert_eq!(updated.grant_mode, GrantMode::ThirtyMinutes);
    }

    #[cfg(unix)]
    #[test]
    fn ssh_key_store_hardens_private_files() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = SshKeyStore::new(tempdir.path());
        store
            .create_or_replace_key("CI".to_string(), GrantMode::Permanent)
            .expect("create key");

        let admin_dir = tempdir.path().join("admin");
        let key_path = admin_dir.join(SSH_KEYS_ENCRYPTION_KEY_FILE);
        let db_path = admin_dir.join(SSH_KEYS_DB_FILE);
        assert_eq!(
            std::fs::metadata(&admin_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&db_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
