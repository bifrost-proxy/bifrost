use std::fmt::Write;
use std::path::Path;

use base64::Engine;
use rand::Rng;
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
}

impl Identity {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(IDENTITY_FILE);

        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            let identity: Identity = serde_json::from_str(&data)?;
            return Ok(identity);
        }

        let identity = Self::generate();
        std::fs::create_dir_all(data_dir)?;
        let json = serde_json::to_string_pretty(&identity)?;
        std::fs::write(&path, json)?;

        Ok(identity)
    }

    fn generate() -> Self {
        let instance_id = uuid::Uuid::new_v4().to_string();
        let device_name = hostname();
        let platform = current_platform().to_string();

        let mut rng = rand::thread_rng();
        let mut pubkey_bytes = [0u8; 32];
        rng.fill(&mut pubkey_bytes);

        let engine = base64::engine::general_purpose::STANDARD;
        let long_term_pubkey = engine.encode(pubkey_bytes);

        let hash = Sha1::digest(pubkey_bytes);
        let mut long_term_pubkey_hash = String::with_capacity(40);
        for byte in hash {
            let _ = write!(long_term_pubkey_hash, "{byte:02x}");
        }

        Self {
            instance_id,
            device_name,
            platform,
            long_term_pubkey,
            long_term_pubkey_hash,
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
