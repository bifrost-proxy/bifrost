use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::limits::MAX_RULE_FILE_BYTES;
use crate::{BifrostError, Result};

pub const RULE_SHARE_QUERY_PARAM: &str = "__bifrost_rule";
pub const RULE_SHARE_PROTOCOL_VERSION: u8 = 1;
pub const RULE_SHARE_CONTENT_HASH_ALGORITHM: &str = "sha256";
pub const RULE_SHARE_IMPORTED_RULE_PREFIX: &str = "share/";
pub const RULE_SHARE_IMPORTED_DESCRIPTION_TITLE: &str = "Imported from a Bifrost rule share link";
pub const RULE_SHARE_IMPORTED_NAME_MARKER: &str = "bifrost-rule-share-name=";
pub const RULE_SHARE_IMPORTED_HASH_MARKER: &str = "bifrost-rule-share-sha256=";

const MAX_RULE_SHARE_BYTES: usize = MAX_RULE_FILE_BYTES as usize;
const MAX_ENCODED_PAYLOAD_BYTES: usize = MAX_RULE_SHARE_BYTES * 2;
const SUPPORTED_SCHEME_HTTP: &str = "http";
const SUPPORTED_SCHEME_HTTPS: &str = "https";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareMode {
    #[default]
    EnableExclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuleShareExclusiveScope {
    #[default]
    MyRules,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSharePayload {
    pub version: u8,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub mode: RuleShareMode,
    #[serde(default)]
    pub exclusive_scope: RuleShareExclusiveScope,
    pub content_hash_algorithm: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleShareUrlParts {
    pub payload: Option<RuleSharePayload>,
    pub clean_url: String,
}

pub fn content_sha256(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn imported_rule_name(source_name: &str) -> String {
    format!("{RULE_SHARE_IMPORTED_RULE_PREFIX}{}", source_name.trim())
}

pub fn imported_rule_description(source_name: &str, content_hash: &str) -> String {
    format!(
        "{RULE_SHARE_IMPORTED_DESCRIPTION_TITLE}\n{RULE_SHARE_IMPORTED_NAME_MARKER}{}\n{RULE_SHARE_IMPORTED_HASH_MARKER}{content_hash}",
        urlencoding::encode(source_name.trim())
    )
}

pub fn imported_rule_source_name(description: Option<&str>) -> Option<String> {
    description.and_then(|description| {
        marker_value(description, RULE_SHARE_IMPORTED_NAME_MARKER).and_then(|value| {
            urlencoding::decode(value)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
    })
}

pub fn imported_rule_content_hash(description: Option<&str>) -> Option<String> {
    description.and_then(|description| {
        marker_value(description, RULE_SHARE_IMPORTED_HASH_MARKER)
            .map(str::trim)
            .map(ToString::to_string)
            .filter(|value| !value.is_empty())
    })
}

pub fn share_payload_name_from_rule(name: &str, description: Option<&str>) -> String {
    let trimmed_name = name.trim();
    if !trimmed_name.starts_with(RULE_SHARE_IMPORTED_RULE_PREFIX) {
        return trimmed_name.to_string();
    }

    if let Some(source_name) = imported_rule_source_name(description) {
        return source_name;
    }

    trimmed_name
        .strip_prefix(RULE_SHARE_IMPORTED_RULE_PREFIX)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(trimmed_name)
        .to_string()
}

pub fn new_rule_share_payload(
    name: impl Into<String>,
    content: impl Into<String>,
) -> Result<RuleSharePayload> {
    let name = name.into();
    let content = content.into();
    let payload = RuleSharePayload {
        version: RULE_SHARE_PROTOCOL_VERSION,
        name,
        content_hash: content_sha256(&content),
        content,
        mode: RuleShareMode::EnableExclusive,
        exclusive_scope: RuleShareExclusiveScope::MyRules,
        content_hash_algorithm: RULE_SHARE_CONTENT_HASH_ALGORITHM.to_string(),
    };
    validate_payload(&payload)?;
    Ok(payload)
}

pub fn encode_rule_share_payload(payload: &RuleSharePayload) -> Result<String> {
    validate_payload(payload)?;
    let bytes = serde_json::to_vec(payload)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_rule_share_payload(encoded: &str) -> Result<RuleSharePayload> {
    if encoded.len() > MAX_ENCODED_PAYLOAD_BYTES {
        return Err(BifrostError::Config(format!(
            "rule share payload is too large: {} bytes",
            encoded.len()
        )));
    }

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|error| {
            BifrostError::Config(format!("invalid rule share base64 payload: {error}"))
        })?;
    if bytes.len() > MAX_RULE_SHARE_BYTES {
        return Err(BifrostError::Config(format!(
            "rule share payload is too large after decoding: {} bytes",
            bytes.len()
        )));
    }

    let payload: RuleSharePayload = serde_json::from_slice(&bytes)?;
    validate_payload(&payload)?;
    Ok(payload)
}

pub fn append_rule_share_query(target_url: &str, payload: &RuleSharePayload) -> Result<String> {
    let encoded = encode_rule_share_payload(payload)?;
    let mut url = parse_http_url(target_url)?;
    remove_rule_share_query(&mut url);
    url.query_pairs_mut()
        .append_pair(RULE_SHARE_QUERY_PARAM, &encoded);
    Ok(url.to_string())
}

pub fn extract_rule_share_query(input_url: &str) -> Result<RuleShareUrlParts> {
    let mut url = parse_http_url(input_url)?;
    let encoded_payload = url
        .query_pairs()
        .find_map(|(key, value)| (key == RULE_SHARE_QUERY_PARAM).then(|| value.into_owned()));
    remove_rule_share_query(&mut url);
    let clean_url = url.to_string();

    let payload = match encoded_payload {
        Some(encoded) => Some(decode_rule_share_payload(&encoded)?),
        None => None,
    };

    Ok(RuleShareUrlParts { payload, clean_url })
}

pub fn validate_payload(payload: &RuleSharePayload) -> Result<()> {
    if payload.version != RULE_SHARE_PROTOCOL_VERSION {
        return Err(BifrostError::Config(format!(
            "unsupported rule share version: {}",
            payload.version
        )));
    }
    if payload.name.trim().is_empty() {
        return Err(BifrostError::Config(
            "rule share name must not be empty".to_string(),
        ));
    }
    if payload.name.contains('/') || payload.name.contains('\\') {
        return Err(BifrostError::Config(
            "rule share name must not contain path separators".to_string(),
        ));
    }
    if payload.content.trim().is_empty() {
        return Err(BifrostError::Config(
            "rule share content must not be empty".to_string(),
        ));
    }
    if payload.content.len() > MAX_RULE_SHARE_BYTES {
        return Err(BifrostError::Config(format!(
            "rule share content is too large: {} bytes",
            payload.content.len()
        )));
    }
    if payload.content_hash_algorithm != RULE_SHARE_CONTENT_HASH_ALGORITHM {
        return Err(BifrostError::Config(format!(
            "unsupported rule share content hash algorithm: {}",
            payload.content_hash_algorithm
        )));
    }
    let expected_hash = content_sha256(&payload.content);
    if payload.content_hash != expected_hash {
        return Err(BifrostError::Config(
            "rule share content hash mismatch".to_string(),
        ));
    }
    Ok(())
}

fn parse_http_url(input_url: &str) -> Result<url::Url> {
    let input_url = input_url.trim();
    let url = match url::Url::parse(input_url) {
        Ok(url) if matches!(url.scheme(), SUPPORTED_SCHEME_HTTP | SUPPORTED_SCHEME_HTTPS) => url,
        Ok(_) if !input_url.contains("://") => {
            url::Url::parse(&format!("{SUPPORTED_SCHEME_HTTP}://{input_url}")).map_err(|error| {
                BifrostError::Config(format!("invalid rule share target URL: {error}"))
            })?
        }
        Ok(url) => {
            return Err(BifrostError::Config(format!(
                "unsupported rule share URL scheme: {}",
                url.scheme()
            )))
        }
        Err(error) if !input_url.contains("://") => {
            url::Url::parse(&format!("{SUPPORTED_SCHEME_HTTP}://{input_url}")).map_err(|_| {
                BifrostError::Config(format!("invalid rule share target URL: {error}"))
            })?
        }
        Err(error) => {
            return Err(BifrostError::Config(format!(
                "invalid rule share target URL: {error}"
            )))
        }
    };
    match url.scheme() {
        SUPPORTED_SCHEME_HTTP | SUPPORTED_SCHEME_HTTPS => Ok(url),
        scheme => Err(BifrostError::Config(format!(
            "unsupported rule share URL scheme: {scheme}"
        ))),
    }
}

fn remove_rule_share_query(url: &mut url::Url) {
    let pairs = url
        .query_pairs()
        .filter(|(key, _)| key != RULE_SHARE_QUERY_PARAM)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();

    if pairs.is_empty() {
        url.set_query(None);
        return;
    }

    url.query_pairs_mut().clear().extend_pairs(pairs);
}

fn marker_value<'a>(description: &'a str, marker: &str) -> Option<&'a str> {
    description
        .lines()
        .find_map(|line| line.trim().strip_prefix(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> RuleSharePayload {
        new_rule_share_payload("demo", "example.com bp://127.0.0.1:3000").unwrap()
    }

    #[test]
    fn encode_decode_round_trip() {
        let payload = sample_payload();
        let encoded = encode_rule_share_payload(&payload).unwrap();
        let decoded = decode_rule_share_payload(&encoded).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn append_and_extract_preserves_site_query_and_fragment() {
        let payload = sample_payload();
        let shared =
            append_rule_share_query("https://example.com/path?a=1&b=2#frag", &payload).unwrap();
        assert!(shared.contains("a=1"));
        assert!(shared.contains(RULE_SHARE_QUERY_PARAM));

        let parts = extract_rule_share_query(&shared).unwrap();
        assert_eq!(parts.payload, Some(payload));
        assert_eq!(parts.clean_url, "https://example.com/path?a=1&b=2#frag");
    }

    #[test]
    fn extract_removes_only_share_query_without_empty_query_suffix() {
        let payload = sample_payload();
        let shared = append_rule_share_query("https://example.com/path#frag", &payload).unwrap();
        let parts = extract_rule_share_query(&shared).unwrap();
        assert_eq!(parts.payload, Some(payload));
        assert_eq!(parts.clean_url, "https://example.com/path#frag");
    }

    #[test]
    fn append_replaces_existing_share_query() {
        let first = sample_payload();
        let second = new_rule_share_payload("demo2", "foo.test direct").unwrap();
        let shared = append_rule_share_query("https://example.com/?x=1", &first).unwrap();
        let replaced = append_rule_share_query(&shared, &second).unwrap();
        assert_eq!(replaced.matches(RULE_SHARE_QUERY_PARAM).count(), 1);
        assert_eq!(
            extract_rule_share_query(&replaced).unwrap().payload,
            Some(second)
        );
    }

    #[test]
    fn append_accepts_schemeless_domain_targets() {
        let payload = sample_payload();
        let shared = append_rule_share_query("a.com/path?site=1", &payload).unwrap();
        assert!(shared.starts_with("http://a.com/path?site=1&"));
        assert!(shared.contains(RULE_SHARE_QUERY_PARAM));
    }

    #[test]
    fn append_accepts_schemeless_localhost_port_targets() {
        let payload = sample_payload();
        let shared = append_rule_share_query("localhost:3000/hello", &payload).unwrap();
        assert!(shared.starts_with("http://localhost:3000/hello?"));
        assert!(shared.contains(RULE_SHARE_QUERY_PARAM));
    }

    #[test]
    fn append_rejects_explicit_non_http_scheme() {
        let payload = sample_payload();
        assert!(append_rule_share_query("ftp://example.com/file", &payload).is_err());
    }

    #[test]
    fn imported_rule_description_round_trips_source_name() {
        let description = imported_rule_description("local debug", "abc123");
        assert_eq!(
            imported_rule_source_name(Some(&description)).as_deref(),
            Some("local debug")
        );
        assert_eq!(
            imported_rule_content_hash(Some(&description)).as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn share_payload_name_strips_import_namespace_and_prefers_source_metadata() {
        let description = imported_rule_description("origin", "abc123");
        assert_eq!(
            share_payload_name_from_rule("share/origin 2", Some(&description)),
            "origin"
        );
        assert_eq!(share_payload_name_from_rule("share/manual", None), "manual");
        assert_eq!(share_payload_name_from_rule("local", None), "local");
    }

    #[test]
    fn decode_rejects_hash_mismatch() {
        let mut payload = sample_payload();
        payload.content.push_str("\nchanged.test reject");
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        assert!(decode_rule_share_payload(&encoded).is_err());
    }

    #[test]
    fn decode_rejects_oversized_encoded_payload() {
        let encoded = "a".repeat(MAX_ENCODED_PAYLOAD_BYTES + 1);
        let err = decode_rule_share_payload(&encoded).unwrap_err();
        assert!(err.to_string().contains("rule share payload is too large"));
    }

    #[test]
    fn decode_rejects_payload_that_decodes_over_limit() {
        let bytes = vec![b'a'; MAX_RULE_SHARE_BYTES + 1];
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let err = decode_rule_share_payload(&encoded).unwrap_err();
        assert!(err
            .to_string()
            .contains("rule share payload is too large after decoding"));
    }

    #[test]
    fn append_rejects_invalid_schemeless_target() {
        let payload = sample_payload();
        let err = append_rule_share_query("exa mple .com/path", &payload).unwrap_err();
        assert!(err.to_string().contains("invalid rule share target URL"));
    }
}
