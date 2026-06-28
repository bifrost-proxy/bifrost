use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Map, Value};

use bifrost_core::{BifrostError, Result};

const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

pub fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&canonicalize(value))
        .map_err(|e| BifrostError::Config(format!("serialize PoP canonical JSON failed: {e}")))
}

pub fn caller_pubkey_b64(key_pair: &Ed25519KeyPair) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(ed25519_spki_der(key_pair.public_key().as_ref()))
}

pub fn sign_envelope(mut body: Value, key_pair: &Ed25519KeyPair) -> Result<Value> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| BifrostError::Config("PoP body must be a JSON object".to_string()))?;
    object.insert("ts".to_string(), Value::from(now_millis()));
    object.insert("nonce".to_string(), Value::String(random_nonce_hex()?));
    object.insert(
        "caller_pubkey".to_string(),
        Value::String(caller_pubkey_b64(key_pair)),
    );
    object.remove("signature");

    let payload = canonical_json(&body)?;
    let signature = key_pair.sign(payload.as_bytes());
    body.as_object_mut().expect("PoP body object").insert(
        "signature".to_string(),
        Value::String(base64::engine::general_purpose::STANDARD.encode(signature.as_ref())),
    );
    Ok(body)
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                if key == "signature" {
                    continue;
                }
                sorted.insert(key.clone(), canonicalize(value));
            }
            let mut out = Map::new();
            for (key, value) in sorted {
                out.insert(key, value);
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn random_nonce_hex() -> Result<String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| BifrostError::Config("generate PoP nonce failed".to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn ed25519_spki_der(public_key: &[u8]) -> Vec<u8> {
    let mut der = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + public_key.len());
    der.extend_from_slice(&ED25519_SPKI_PREFIX);
    der.extend_from_slice(public_key);
    der
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_recursively_and_omits_signature() {
        let value = serde_json::json!({
            "z": 1,
            "signature": "ignored",
            "a": { "d": 4, "b": 2 },
            "c": [3, { "y": 2, "x": 1 }]
        });

        let rendered = canonical_json(&value).expect("canonical JSON");
        assert_eq!(
            rendered,
            r#"{"a":{"b":2,"d":4},"c":[3,{"x":1,"y":2}],"z":1}"#
        );
    }

    #[test]
    fn sign_envelope_adds_required_fields() {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("keypair");
        let signed = sign_envelope(serde_json::json!({ "client_instance_id": "c1" }), &key_pair)
            .expect("sign");

        assert!(signed.get("ts").and_then(Value::as_u64).is_some());
        assert_eq!(
            signed.get("nonce").and_then(Value::as_str).unwrap().len(),
            32
        );
        assert!(signed
            .get("caller_pubkey")
            .and_then(Value::as_str)
            .is_some());
        assert!(signed.get("signature").and_then(Value::as_str).is_some());
    }
}
