use std::path::Path;

use base64::Engine;
use bifrost_core::{BifrostError, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ENVELOPE_VERSION: u32 = 1;
const ENVELOPE_PREFIX: &str = "bifrost-local-secret:";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalSecretEnvelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone)]
pub struct LocalSecretKey {
    bytes: [u8; 32],
}

impl LocalSecretKey {
    pub fn for_data_dir(data_dir: &Path) -> Self {
        Self {
            bytes: derive_device_bound_key(data_dir),
        }
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
        let envelope: LocalSecretEnvelope = serde_json::from_str(json)
            .map_err(|e| BifrostError::Config(format!("parse local config secret: {e}")))?;
        if envelope.version != ENVELOPE_VERSION {
            return Err(BifrostError::Config(format!(
                "unsupported local config secret version: {}",
                envelope.version
            )));
        }

        let unbound = UnboundKey::new(&AES_256_GCM, &self.bytes).map_err(|_| {
            BifrostError::Config("initialize local config secret decryption failed".to_string())
        })?;
        let key = LessSafeKey::new(unbound);
        let nonce_raw = base64::engine::general_purpose::STANDARD
            .decode(envelope.nonce)
            .map_err(|e| BifrostError::Config(format!("decode local config secret nonce: {e}")))?;
        let nonce: [u8; NONCE_LEN] = nonce_raw.try_into().map_err(|_| {
            BifrostError::Config("local config secret nonce must be 12 bytes".to_string())
        })?;
        let mut ciphertext = base64::engine::general_purpose::STANDARD
            .decode(envelope.ciphertext)
            .map_err(|e| {
                BifrostError::Config(format!("decode local config secret ciphertext: {e}"))
            })?;
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut ciphertext,
            )
            .map_err(|_| BifrostError::Config("decrypt local config secret failed".to_string()))?;
        String::from_utf8(plaintext.to_vec()).map_err(|e| {
            BifrostError::Config(format!("local config secret is not valid UTF-8: {e}"))
        })
    }
}

pub fn is_encrypted_local_secret(value: &str) -> bool {
    value.starts_with(ENVELOPE_PREFIX)
}

fn derive_device_bound_key(data_dir: &Path) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"bifrost-local-config-secret-v1");
    for material in device_fingerprint_material(data_dir) {
        hasher.update([0]);
        hasher.update(material.as_bytes());
    }
    hasher.finalize().into()
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
        let key = LocalSecretKey::for_data_dir(dir.path());

        let plaintext = "p@ssw0rd";
        let encrypted = key.encrypt_string(plaintext).unwrap();

        assert!(is_encrypted_local_secret(&encrypted));
        assert!(!encrypted.contains(plaintext));
        assert_eq!(key.decrypt_string(&encrypted).unwrap(), plaintext);
    }

    #[test]
    fn decrypt_plaintext_preserves_legacy_value() {
        let dir = tempfile::tempdir().unwrap();
        let key = LocalSecretKey::for_data_dir(dir.path());

        assert_eq!(key.decrypt_string("legacy").unwrap(), "legacy");
        assert!(!is_encrypted_local_secret("legacy"));
    }

    #[test]
    fn key_is_stable_for_same_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let first = LocalSecretKey::for_data_dir(dir.path());
        let second = LocalSecretKey::for_data_dir(dir.path());
        let encrypted = first.encrypt_string("same-device").unwrap();

        assert_eq!(second.decrypt_string(&encrypted).unwrap(), "same-device");
    }
}
