use std::path::Path;

use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use bifrost_core::Result;

const IDENTITY_FILE: &str = "remote_invoke_client.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub instance_id: String,
    pub device_name: String,
    pub platform: String,
    pub long_term_pubkey: String,
    pub long_term_pubkey_hash: String,
    pub long_term_private_key_pkcs8: String,
}

impl Identity {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(IDENTITY_FILE);

        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            let stored: StoredIdentity = serde_json::from_str(&data)?;
            if let Some(identity) = Self::from_stored(stored)? {
                harden_private_dir(data_dir)?;
                harden_private_file(&path)?;
                return Ok(identity);
            }
        }

        let identity = Self::generate();
        std::fs::create_dir_all(data_dir)?;
        harden_private_dir(data_dir)?;
        let json = serde_json::to_string_pretty(&identity)?;
        std::fs::write(&path, json)?;
        harden_private_file(&path)?;

        Ok(identity)
    }

    fn generate() -> Self {
        let instance_id = uuid::Uuid::new_v4().to_string();
        let device_name = hostname();
        let platform = current_platform().to_string();

        let rng = SystemRandom::new();
        let pkcs8 =
            Ed25519KeyPair::generate_pkcs8(&rng).expect("failed to generate ed25519 keypair");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .expect("failed to parse generated ed25519 keypair");

        let public_key_der = ed25519_public_key_to_spki_der(key_pair.public_key().as_ref());
        let engine = base64::engine::general_purpose::STANDARD;
        let long_term_pubkey = engine.encode(&public_key_der);
        let long_term_private_key_pkcs8 = engine.encode(pkcs8.as_ref());

        Self {
            instance_id,
            device_name,
            platform,
            long_term_pubkey,
            long_term_pubkey_hash: sha1_hex(&public_key_der),
            long_term_private_key_pkcs8,
        }
    }

    pub fn to_client_identity(&self) -> super::types::ClientIdentity {
        super::types::ClientIdentity {
            instance_id: self.instance_id.clone(),
            device_name: self.device_name.clone(),
            platform: self.platform.clone(),
            long_term_pubkey: self.long_term_pubkey.clone(),
            long_term_pubkey_hash: self.long_term_pubkey_hash.clone(),
        }
    }

    pub fn sign_registration_payload(&self, payload: &[u8]) -> Result<String> {
        let engine = base64::engine::general_purpose::STANDARD;
        let pkcs8 = engine
            .decode(&self.long_term_private_key_pkcs8)
            .map_err(|e| {
                std::io::Error::other(format!("invalid remote invoke private key: {e}"))
            })?;
        let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
            .map_err(|_| std::io::Error::other("invalid remote invoke private key"))?;
        Ok(engine.encode(key_pair.sign(payload).as_ref()))
    }

    fn from_stored(stored: StoredIdentity) -> Result<Option<Self>> {
        let Some(long_term_private_key_pkcs8) = stored.long_term_private_key_pkcs8 else {
            return Ok(None);
        };

        let engine = base64::engine::general_purpose::STANDARD;
        let pkcs8 = engine.decode(&long_term_private_key_pkcs8).map_err(|e| {
            std::io::Error::other(format!("invalid remote invoke private key: {e}"))
        })?;
        let key_pair = match Ed25519KeyPair::from_pkcs8(&pkcs8) {
            Ok(pair) => pair,
            Err(_) => return Ok(None),
        };

        let public_key_der = ed25519_public_key_to_spki_der(key_pair.public_key().as_ref());
        let expected_pubkey = engine.encode(&public_key_der);
        if stored.long_term_pubkey != expected_pubkey {
            return Ok(None);
        }

        Ok(Some(Self {
            instance_id: stored.instance_id,
            device_name: stored.device_name,
            platform: stored.platform,
            long_term_pubkey: stored.long_term_pubkey,
            long_term_pubkey_hash: if stored.long_term_pubkey_hash.is_empty() {
                sha1_hex(&public_key_der)
            } else {
                stored.long_term_pubkey_hash
            },
            long_term_private_key_pkcs8,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredIdentity {
    pub instance_id: String,
    pub device_name: String,
    pub platform: String,
    pub long_term_pubkey: String,
    #[serde(default)]
    pub long_term_pubkey_hash: String,
    #[serde(default)]
    pub long_term_private_key_pkcs8: Option<String>,
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

fn sha1_hex(data: &[u8]) -> String {
    let digest = Sha1::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

#[cfg(unix)]
fn harden_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn harden_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ED25519};

    #[test]
    fn generated_identity_can_sign_registration_payload() {
        let identity = Identity::generate();
        let payload = br#"["bifrost-remote-register-v1","challenge-id","challenge","client-id","device","macos","0.0.0-test","pubkey",1700000000]"#;
        let signature = identity
            .sign_registration_payload(payload)
            .expect("signature should be generated");

        let engine = base64::engine::general_purpose::STANDARD;
        let public_key_der = engine
            .decode(&identity.long_term_pubkey)
            .expect("public key should decode");
        let signature = engine.decode(signature).expect("signature should decode");
        let raw_public_key = &public_key_der[12..];

        UnparsedPublicKey::new(&ED25519, raw_public_key)
            .verify(payload, &signature)
            .expect("signature should verify");
    }

    #[test]
    fn load_or_create_persists_private_key_material() {
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let first = Identity::load_or_create(tempdir.path()).expect("identity should be created");
        let second = Identity::load_or_create(tempdir.path()).expect("identity should reload");

        assert_eq!(first.instance_id, second.instance_id);
        assert_eq!(first.long_term_pubkey, second.long_term_pubkey);
        assert_eq!(
            first.long_term_private_key_pkcs8,
            second.long_term_private_key_pkcs8
        );
    }
}
