use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;

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

/// P0-A: sign a caller ephemeral X25519 public key with the caller PoP Ed25519
/// long-term key. The signed message MUST be byte-identical to what the target
/// reconstructs via `types::build_ephemeral_signature_payload`, so the payload
/// layout is fixed here and mirrored on the admin side. `binding_id` and
/// `signer_instance_id` are both the caller fingerprint (a value both peers see
/// via the relay-forwarded pairing request); `peer_fingerprint` is empty because
/// the caller has no trusted knowledge of the target long-term key at pair time.
pub fn sign_ephemeral_pub(
    key_pair: &Ed25519KeyPair,
    caller_fingerprint: &str,
    caller_ephemeral_pub_b64: &str,
) -> String {
    let payload = ephemeral_signature_payload(caller_fingerprint, caller_ephemeral_pub_b64);
    let signature = key_pair.sign(payload.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(signature.as_ref())
}

/// Domain-separation tag; MUST equal `types::EPHEMERAL_SIG_DOMAIN`.
const EPHEMERAL_SIG_DOMAIN: &str = "bifrost-remote-ephemeral-v1";

/// Canonical JSON payload signed over the caller ephemeral pubkey. Field order
/// and values MUST match `types::build_ephemeral_signature_payload` exactly.
fn ephemeral_signature_payload(caller_fingerprint: &str, caller_ephemeral_pub_b64: &str) -> String {
    serde_json::json!([
        EPHEMERAL_SIG_DOMAIN,
        caller_fingerprint,
        caller_fingerprint,
        "",
        caller_ephemeral_pub_b64,
        0u64,
    ])
    .to_string()
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
        Value::String(s) => Value::String(s.nfc().collect()),
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

    #[test]
    fn canonical_json_normalizes_strings_to_nfc() {
        let decomposed = "Cafe\u{0301}";
        let rendered = canonical_json(&serde_json::json!({
            "text": decomposed,
            "nested": { "path": decomposed }
        }))
        .expect("canonical JSON");

        assert_eq!(rendered, r#"{"nested":{"path":"Café"},"text":"Café"}"#);
    }

    // P0-A: the caller signature MUST verify against the exact payload the
    // target reconstructs. This test rebuilds that payload verbatim (domain,
    // fp, fp, empty peer fp, ephemeral pub, ts=0) and verifies the signature
    // with the caller PoP public key, catching any caller/target drift.
    #[test]
    fn sign_ephemeral_pub_verifies_against_target_payload() {
        use ring::signature::{UnparsedPublicKey, ED25519};
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("keypair");
        let fp = "cafefeed";
        let eph = "oaNKFHB0HF2375h+0cywBEOqPbh1zUga5nvlpAaxhk0=";
        let sig_b64 = sign_ephemeral_pub(&key_pair, fp, eph);
        let sig = base64::engine::general_purpose::STANDARD
            .decode(sig_b64)
            .expect("decode sig");
        // Rebuild the exact target-side payload shape.
        let expected_payload =
            serde_json::json!(["bifrost-remote-ephemeral-v1", fp, fp, "", eph, 0u64,]).to_string();
        UnparsedPublicKey::new(&ED25519, key_pair.public_key().as_ref())
            .verify(expected_payload.as_bytes(), &sig)
            .expect("signature must verify against target payload");
        // A tampered ephemeral pub must NOT verify with the original signature.
        let tampered = serde_json::json!([
            "bifrost-remote-ephemeral-v1",
            fp,
            fp,
            "",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            0u64,
        ])
        .to_string();
        assert!(
            UnparsedPublicKey::new(&ED25519, key_pair.public_key().as_ref())
                .verify(tampered.as_bytes(), &sig)
                .is_err()
        );
    }
}
