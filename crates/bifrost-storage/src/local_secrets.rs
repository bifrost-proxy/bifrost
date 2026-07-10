use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use base64::Engine;
use bifrost_core::{BifrostError, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ENVELOPE_VERSION: u32 = 1;
const ENVELOPE_PREFIX: &str = "bifrost-local-secret:";
const KEY_FILE_NAME: &str = "local_config_secret.key";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalSecretEnvelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone)]
pub struct LocalSecretKey {
    bytes: [u8; 32],
    legacy_device_bound_bytes: [u8; 32],
}

impl LocalSecretKey {
    pub fn for_data_dir(data_dir: &Path) -> Result<Self> {
        Ok(Self {
            bytes: load_or_create_key(data_dir)?,
            // PR #362 briefly wrote envelopes with a fingerprint-derived key.
            // Keep a read-only fallback so those files migrate on their next save.
            legacy_device_bound_bytes: derive_legacy_device_bound_key(data_dir),
        })
    }

    pub fn encrypt_string(&self, plaintext: &str) -> Result<String> {
        let unbound = UnboundKey::new(&AES_256_GCM, &self.bytes).map_err(|_| {
            BifrostError::Config("initialize local config secret encryption failed".to_string())
        })?;
        let key = LessSafeKey::new(unbound);
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| {
            BifrostError::Config("generate local config secret nonce failed".to_string())
        })?;
        let mut ciphertext = plaintext.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| BifrostError::Config("encrypt local config secret failed".to_string()))?;

        let envelope = LocalSecretEnvelope {
            version: ENVELOPE_VERSION,
            nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
            ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
        };
        let json = serde_json::to_string(&envelope)
            .map_err(|e| BifrostError::Config(format!("serialize local config secret: {e}")))?;
        Ok(format!("{ENVELOPE_PREFIX}{json}"))
    }

    pub fn decrypt_string(&self, encoded: &str) -> Result<String> {
        let Some(json) = encoded.strip_prefix(ENVELOPE_PREFIX) else {
            return Ok(encoded.to_string());
        };
        let envelope: LocalSecretEnvelope = match serde_json::from_str(json) {
            Ok(envelope) => envelope,
            // Plaintext passwords predate this envelope format and may legally
            // begin with the reserved-looking prefix. Preserve them; every save
            // now encrypts the in-memory plaintext unconditionally.
            Err(_) => return Ok(encoded.to_string()),
        };
        if envelope.version != ENVELOPE_VERSION {
            return Err(BifrostError::Config(format!(
                "unsupported local config secret version: {}",
                envelope.version
            )));
        }

        let nonce_raw = base64::engine::general_purpose::STANDARD
            .decode(envelope.nonce)
            .map_err(|e| BifrostError::Config(format!("decode local config secret nonce: {e}")))?;
        let nonce: [u8; NONCE_LEN] = nonce_raw.try_into().map_err(|_| {
            BifrostError::Config("local config secret nonce must be 12 bytes".to_string())
        })?;
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(envelope.ciphertext)
            .map_err(|e| {
                BifrostError::Config(format!("decode local config secret ciphertext: {e}"))
            })?;
        let plaintext = decrypt_bytes(&self.bytes, nonce, &ciphertext)
            .or_else(|_| decrypt_bytes(&self.legacy_device_bound_bytes, nonce, &ciphertext))?;
        String::from_utf8(plaintext.to_vec()).map_err(|e| {
            BifrostError::Config(format!("local config secret is not valid UTF-8: {e}"))
        })
    }
}

#[cfg(test)]
fn is_encrypted_local_secret(value: &str) -> bool {
    value.starts_with(ENVELOPE_PREFIX)
}

fn load_or_create_key(data_dir: &Path) -> Result<[u8; 32]> {
    std::fs::create_dir_all(data_dir).map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!(
            "create local config secret key directory {}: {error}",
            data_dir.display()
        )))
    })?;
    let path = data_dir.join(KEY_FILE_NAME);
    match read_key_file(&path) {
        Ok(Some(key)) => return Ok(key),
        Ok(None) => {}
        Err(error) => return Err(error),
    }

    let mut key = [0u8; 32];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| BifrostError::Config("generate local config secret key failed".to_string()))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(encoded.as_bytes()).map_err(|error| {
                BifrostError::Io(std::io::Error::other(format!(
                    "write local config secret key {}: {error}",
                    path.display()
                )))
            })?;
            file.write_all(b"\n").map_err(|error| {
                BifrostError::Io(std::io::Error::other(format!(
                    "finish local config secret key {}: {error}",
                    path.display()
                )))
            })?;
            harden_private_file(&path)?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_key_file(&path)?
            .ok_or_else(|| {
                BifrostError::Config(format!(
                    "local config secret key disappeared during creation: {}",
                    path.display()
                ))
            }),
        Err(error) => Err(BifrostError::Io(std::io::Error::other(format!(
            "create local config secret key {}: {error}",
            path.display()
        )))),
    }
}

fn read_key_file(path: &Path) -> Result<Option<[u8; 32]>> {
    if !path.exists() {
        return Ok(None);
    }
    let encoded = std::fs::read_to_string(path).map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!(
            "read local config secret key {}: {error}",
            path.display()
        )))
    })?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| {
            BifrostError::Config(format!(
                "decode local config secret key {}: {error}",
                path.display()
            ))
        })?;
    let key = raw.try_into().map_err(|_| {
        BifrostError::Config(format!(
            "local config secret key {} must contain 32 bytes",
            path.display()
        ))
    })?;
    harden_private_file(path)?;
    Ok(Some(key))
}

fn decrypt_bytes(
    key_bytes: &[u8; 32],
    nonce: [u8; NONCE_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let unbound = UnboundKey::new(&AES_256_GCM, key_bytes).map_err(|_| {
        BifrostError::Config("initialize local config secret decryption failed".to_string())
    })?;
    let key = LessSafeKey::new(unbound);
    let mut ciphertext = ciphertext.to_vec();
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| BifrostError::Config("decrypt local config secret failed".to_string()))?;
    Ok(plaintext.to_vec())
}

fn derive_legacy_device_bound_key(data_dir: &Path) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"bifrost-local-config-secret-v1");
    for material in device_fingerprint_material(data_dir) {
        hasher.update([0]);
        hasher.update(material.as_bytes());
    }
    hasher.finalize().into()
}

#[cfg(unix)]
fn harden_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        BifrostError::Io(std::io::Error::other(format!(
            "chmod 0600 {}: {error}",
            path.display()
        )))
    })
}

#[cfg(not(unix))]
fn harden_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn device_fingerprint_material(data_dir: &Path) -> Vec<String> {
    let mut materials = Vec::new();
    materials.push(format!("data_dir={}", data_dir.display()));
    if let Some(hostname) = hostname() {
        materials.push(format!("hostname={hostname}"));
    }
    for key in ["USER", "USERNAME", "HOME", "USERPROFILE"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                materials.push(format!("{key}={value}"));
            }
        }
    }
    for path in machine_id_paths() {
        if let Ok(value) = std::fs::read_to_string(path) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                materials.push(format!("{}={trimmed}", path.display()));
            }
        }
    }
    materials
}

fn machine_id_paths() -> Vec<&'static Path> {
    vec![
        Path::new("/etc/machine-id"),
        Path::new("/var/lib/dbus/machine-id"),
        Path::new("/etc/hostid"),
    ]
}

fn hostname() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output.stdout))
        .and_then(|stdout| String::from_utf8(stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip_and_marks_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let key = LocalSecretKey::for_data_dir(dir.path()).unwrap();

        let plaintext = "p@ssw0rd";
        let encrypted = key.encrypt_string(plaintext).unwrap();

        assert!(is_encrypted_local_secret(&encrypted));
        assert!(!encrypted.contains(plaintext));
        assert_eq!(key.decrypt_string(&encrypted).unwrap(), plaintext);
    }

    #[test]
    fn decrypt_plaintext_preserves_legacy_value() {
        let dir = tempfile::tempdir().unwrap();
        let key = LocalSecretKey::for_data_dir(dir.path()).unwrap();

        assert_eq!(key.decrypt_string("legacy").unwrap(), "legacy");
        assert!(!is_encrypted_local_secret("legacy"));
    }

    #[test]
    fn key_is_stable_for_same_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let first = LocalSecretKey::for_data_dir(dir.path()).unwrap();
        let second = LocalSecretKey::for_data_dir(dir.path()).unwrap();
        let encrypted = first.encrypt_string("same-device").unwrap();

        assert_eq!(second.decrypt_string(&encrypted).unwrap(), "same-device");
    }

    #[test]
    fn fingerprint_derived_envelope_is_read_for_migration() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_bytes = derive_legacy_device_bound_key(dir.path());
        let legacy = LocalSecretKey {
            bytes: legacy_bytes,
            legacy_device_bound_bytes: legacy_bytes,
        };
        let encrypted = legacy.encrypt_string("legacy-device-envelope").unwrap();
        let current = LocalSecretKey::for_data_dir(dir.path()).unwrap();

        assert_eq!(
            current.decrypt_string(&encrypted).unwrap(),
            "legacy-device-envelope"
        );
    }

    #[test]
    fn plaintext_reserved_prefix_is_not_misparsed() {
        let dir = tempfile::tempdir().unwrap();
        let key = LocalSecretKey::for_data_dir(dir.path()).unwrap();
        let plaintext = "bifrost-local-secret:not-json";

        assert_eq!(key.decrypt_string(plaintext).unwrap(), plaintext);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_key_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        LocalSecretKey::for_data_dir(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join(KEY_FILE_NAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o600);
    }
}
