use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use parking_lot::Mutex;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use bifrost_core::{BifrostError, Result};

const GRANT_CRYPTO_STORE_FILE: &str = "remote_invoke_grant_crypto.json";
const GRANT_CRYPTO_STORE_KEY_FILE: &str = "remote_invoke_grant_crypto.key";
const GRANT_CRYPTO_STORE_VERSION: u32 = 1;
const GRANT_CRYPTO_SECRET_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGrantCryptoMaterial {
    pub shared_secret: Vec<u8>,
    pub caller_ephemeral_pub: String,
    pub client_ephemeral_pub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantCryptoStoreFile {
    version: u32,
    entries: Vec<PersistedGrantCryptoEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedGrantCryptoEntry {
    grant_id: String,
    relay_url: String,
    caller_ephemeral_pub: String,
    client_ephemeral_pub: String,
    shared_secret_encrypted: String,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalSecretEnvelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

pub struct GrantCryptoStore {
    file_path: PathBuf,
    key_path: PathBuf,
    lock: Mutex<()>,
}

impl GrantCryptoStore {
    pub fn new(data_dir: &Path) -> Self {
        let admin_dir = data_dir.join("admin");
        Self {
            file_path: admin_dir.join(GRANT_CRYPTO_STORE_FILE),
            key_path: admin_dir.join(GRANT_CRYPTO_STORE_KEY_FILE),
            lock: Mutex::new(()),
        }
    }

    pub fn load_for_relay(
        &self,
        relay_url: &str,
    ) -> Result<HashMap<String, StoredGrantCryptoMaterial>> {
        let _guard = self.lock.lock();
        let file = self.read_store_file()?;
        let mut grants = HashMap::new();
        for entry in file.entries {
            if entry.relay_url != relay_url {
                continue;
            }
            grants.insert(
                entry.grant_id,
                StoredGrantCryptoMaterial {
                    shared_secret: self.decrypt_secret(&entry.shared_secret_encrypted)?,
                    caller_ephemeral_pub: entry.caller_ephemeral_pub,
                    client_ephemeral_pub: entry.client_ephemeral_pub,
                },
            );
        }
        Ok(grants)
    }

    pub fn upsert(
        &self,
        relay_url: &str,
        grant_id: &str,
        material: &StoredGrantCryptoMaterial,
    ) -> Result<()> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let encrypted_secret = self.encrypt_secret(&material.shared_secret)?;
        let updated_at = now_millis();

        if let Some(existing) = file
            .entries
            .iter_mut()
            .find(|entry| entry.grant_id == grant_id && entry.relay_url == relay_url)
        {
            existing.caller_ephemeral_pub = material.caller_ephemeral_pub.clone();
            existing.client_ephemeral_pub = material.client_ephemeral_pub.clone();
            existing.shared_secret_encrypted = encrypted_secret;
            existing.updated_at = updated_at;
        } else {
            file.entries.push(PersistedGrantCryptoEntry {
                grant_id: grant_id.to_string(),
                relay_url: relay_url.to_string(),
                caller_ephemeral_pub: material.caller_ephemeral_pub.clone(),
                client_ephemeral_pub: material.client_ephemeral_pub.clone(),
                shared_secret_encrypted: encrypted_secret,
                updated_at,
            });
        }

        self.write_store_file(&file)
    }

    pub fn remove(&self, grant_id: &str) -> Result<()> {
        let _guard = self.lock.lock();
        let mut file = self.read_store_file()?;
        let before = file.entries.len();
        file.entries.retain(|entry| entry.grant_id != grant_id);
        if before == file.entries.len() {
            return Ok(());
        }
        self.write_store_file(&file)
    }

    fn read_store_file(&self) -> Result<GrantCryptoStoreFile> {
        if !self.file_path.exists() {
            return Ok(GrantCryptoStoreFile {
                version: GRANT_CRYPTO_STORE_VERSION,
                entries: Vec::new(),
            });
        }

        let content = std::fs::read_to_string(&self.file_path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                self.file_path.display()
            )))
        })?;
        let file: GrantCryptoStoreFile =
            match serde_json::from_str::<GrantCryptoStoreFile>(&content) {
                Ok(file) if file.version == GRANT_CRYPTO_STORE_VERSION => file,
                Ok(file) => {
                    self.reset_store_files()?;
                    return Err(BifrostError::Config(format!(
                        "reset incompatible grant crypto store version {}",
                        file.version
                    )));
                }
                Err(e) => {
                    self.reset_store_files()?;
                    return Err(BifrostError::Config(format!(
                        "reset unreadable grant crypto store {}: {e}",
                        self.file_path.display()
                    )));
                }
            };
        Ok(file)
    }

    fn write_store_file(&self, file: &GrantCryptoStoreFile) -> Result<()> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }
        let content = serde_json::to_string_pretty(file)
            .map_err(|e| BifrostError::Config(format!("serialize grant crypto store: {e}")))?;
        std::fs::write(&self.file_path, content).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.file_path.display()
            )))
        })?;
        Ok(())
    }

    fn load_or_create_key(&self) -> Result<[u8; 32]> {
        if self.key_path.exists() {
            let encoded = std::fs::read_to_string(&self.key_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "read {}: {e}",
                    self.key_path.display()
                )))
            })?;
            let raw = match base64::engine::general_purpose::STANDARD.decode(encoded.trim()) {
                Ok(raw) => raw,
                Err(e) => {
                    self.reset_store_files()?;
                    return Err(BifrostError::Config(format!(
                        "reset invalid grant crypto key {}: {e}",
                        self.key_path.display()
                    )));
                }
            };
            let key: [u8; 32] = match raw.try_into() {
                Ok(key) => key,
                Err(_) => {
                    self.reset_store_files()?;
                    return Err(BifrostError::Config(format!(
                        "reset invalid grant crypto key length {}",
                        self.key_path.display()
                    )));
                }
            };
            return Ok(key);
        }

        if let Some(parent) = self.key_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )))
            })?;
        }

        let mut key = [0u8; 32];
        SystemRandom::new().fill(&mut key).map_err(|_| {
            BifrostError::Config("generate grant crypto store key failed".to_string())
        })?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        std::fs::write(&self.key_path, format!("{encoded}\n")).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "write {}: {e}",
                self.key_path.display()
            )))
        })?;
        Ok(key)
    }

    fn reset_store_files(&self) -> Result<()> {
        if self.file_path.exists() {
            std::fs::remove_file(&self.file_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "remove {}: {e}",
                    self.file_path.display()
                )))
            })?;
        }
        if self.key_path.exists() {
            std::fs::remove_file(&self.key_path).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "remove {}: {e}",
                    self.key_path.display()
                )))
            })?;
        }
        Ok(())
    }

    fn encrypt_secret(&self, plaintext: &[u8]) -> Result<String> {
        let key_bytes = self.load_or_create_key()?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            BifrostError::Config("initialize grant crypto store encryption failed".to_string())
        })?;
        let key = LessSafeKey::new(unbound);
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| BifrostError::Config("generate grant crypto nonce failed".to_string()))?;
        let mut ciphertext = plaintext.to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| BifrostError::Config("encrypt grant crypto secret failed".to_string()))?;

        serde_json::to_string(&LocalSecretEnvelope {
            version: GRANT_CRYPTO_SECRET_VERSION,
            nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
            ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
        })
        .map_err(|e| BifrostError::Config(format!("serialize grant crypto secret failed: {e}")))
    }

    fn decrypt_secret(&self, encoded: &str) -> Result<Vec<u8>> {
        let envelope: LocalSecretEnvelope = serde_json::from_str(encoded)
            .map_err(|e| BifrostError::Config(format!("parse grant crypto secret failed: {e}")))?;
        if envelope.version != GRANT_CRYPTO_SECRET_VERSION {
            return Err(BifrostError::Config(format!(
                "unsupported grant crypto secret version: {}",
                envelope.version
            )));
        }

        let key_bytes = self.load_or_create_key()?;
        let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            BifrostError::Config("initialize grant crypto store decryption failed".to_string())
        })?;
        let key = LessSafeKey::new(unbound);
        let nonce_raw = base64::engine::general_purpose::STANDARD
            .decode(envelope.nonce)
            .map_err(|e| BifrostError::Config(format!("decode grant crypto nonce failed: {e}")))?;
        let nonce: [u8; NONCE_LEN] = nonce_raw
            .try_into()
            .map_err(|_| BifrostError::Config("grant crypto nonce must be 12 bytes".to_string()))?;
        let mut ciphertext = base64::engine::general_purpose::STANDARD
            .decode(envelope.ciphertext)
            .map_err(|e| {
                BifrostError::Config(format!("decode grant crypto ciphertext failed: {e}"))
            })?;
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut ciphertext,
            )
            .map_err(|_| BifrostError::Config("decrypt grant crypto secret failed".to_string()))?;
        Ok(plaintext.to_vec())
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{GrantCryptoStore, StoredGrantCryptoMaterial};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bifrost-grant-crypto-store-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn test_grant_crypto_store_roundtrip_for_relay() {
        let data_dir = temp_dir("roundtrip");
        let store = GrantCryptoStore::new(&data_dir);
        let material = StoredGrantCryptoMaterial {
            shared_secret: vec![7u8; 32],
            caller_ephemeral_pub: "caller-ephemeral".to_string(),
            client_ephemeral_pub: "client-ephemeral".to_string(),
        };

        store
            .upsert("https://relay.example", "grant-1", &material)
            .expect("persist grant crypto");
        store
            .upsert(
                "https://other-relay.example",
                "grant-2",
                &StoredGrantCryptoMaterial {
                    shared_secret: vec![9u8; 32],
                    caller_ephemeral_pub: "other-caller".to_string(),
                    client_ephemeral_pub: "other-client".to_string(),
                },
            )
            .expect("persist other relay grant crypto");

        let loaded = store
            .load_for_relay("https://relay.example")
            .expect("load relay grants");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("grant-1"), Some(&material));

        store.remove("grant-1").expect("remove grant crypto");
        let loaded_after_remove = store
            .load_for_relay("https://relay.example")
            .expect("reload relay grants after remove");
        assert!(loaded_after_remove.is_empty());

        let _ = std::fs::remove_dir_all(data_dir);
    }
}
