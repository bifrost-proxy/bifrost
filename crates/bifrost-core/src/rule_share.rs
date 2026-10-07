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

/// Detect the actual query key, including percent-encoded spellings.
pub fn has_rule_share_query(input_url: &str) -> bool {
    input_url.split_once('?').is_some_and(|(_, query)| {
        let query = query.split('#').next().unwrap_or_default();
        url::form_urlencoded::parse(query.as_bytes()).any(|(key, _)| key == RULE_SHARE_QUERY_PARAM)
    })
}

pub fn extract_rule_share_query(input_url: &str) -> Result<RuleShareUrlParts> {
    let mut url = parse_http_url(input_url)?;
    let mut payloads = url
        .query_pairs()
        .filter(|(key, _)| key == RULE_SHARE_QUERY_PARAM)
        .map(|(_, value)| value.into_owned());
    let encoded_payload = payloads.next();
    if payloads.next().is_some() {
        return Err(BifrostError::Config(
            "Duplicate __bifrost_rule parameters. Generate a new link with bifrost rule share, then run bifrost rule verify.".to_string(),
        ));
    }
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
        return Err(BifrostError::Config(format!(
            "rule share content hash mismatch: declared {}, actual {expected_hash}. The rule content was changed after hashing; regenerate the link with bifrost rule share and run bifrost rule verify before sharing.",
            payload.content_hash,
        )));
    }
    validate_shared_rule_content(&payload.content)
}

/// Keep generation, verification and import on the same syntax validation path.
/// @ references remain receiver-local, as in the existing share import contract.
pub fn validate_shared_rule_content(content: &str) -> Result<()> {
    let validation_content = content
        .lines()
        .map(|line| {
            if crate::rule_reference_name(line).is_some() {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let errors = crate::validate_rules(&validation_content);
    if errors.is_empty() {
        return Ok(());
    }
    let details = errors
        .iter()
        .map(|error| {
            format!(
                "{} line {}:{}-{}: {}{}",
                error.code.as_deref().unwrap_or("syntax"),
                error.line,
                error.start_column,
                error.end_column,
                error.message,
                error
                    .suggestion
                    .as_ref()
                    .map(|s| format!(" Suggestion: {s}"))
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(BifrostError::Rule(format!(
        "invalid shared rule content:\n{details}\nFix the rule content, regenerate with bifrost rule share, then run bifrost rule verify."
    )))
}

/// Offline verification: does not import rules or contact the target website.
pub fn verify_rule_share_url(input_url: &str) -> Result<RuleShareUrlParts> {
    let parts = extract_rule_share_query(input_url)?;
    if parts.payload.is_none() {
        return Err(BifrostError::Config(
            "Missing __bifrost_rule parameter. Verify the complete URL generated by bifrost rule share.".to_string(),
        ));
    }
    Ok(parts)
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
        let second = new_rule_share_payload("demo2", "foo.test passthrough://").unwrap();
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
    fn unchecked_encoded(payload: &RuleSharePayload) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(payload).unwrap())
    }

    #[test]
    fn verify_reports_hash_mismatch_with_repair_guidance() {
        let mut payload = sample_payload();
        payload.content.push_str("\nchanged.test status://201");
        let url = format!(
            "https://example.com/?__bifrost_rule={}",
            unchecked_encoded(&payload)
        );
        let error = verify_rule_share_url(&url).unwrap_err().to_string();
        assert!(error.contains("hash mismatch"));
        assert!(error.contains(&content_sha256(&payload.content)));
        assert!(error.contains("bifrost rule verify"));
    }

    #[test]
    fn generation_and_verification_reject_invalid_rule_syntax() {
        let invalid = "example.test unknownProtocol://value";
        let error = new_rule_share_payload("broken", invalid)
            .unwrap_err()
            .to_string();
        assert!(error.contains("line 1:"), "{error}");
        assert!(error.contains("Suggestion:"), "{error}");
        let mut payload = sample_payload();
        payload.content = invalid.into();
        payload.content_hash = content_sha256(invalid);
        let url = format!(
            "https://example.com/?__bifrost_rule={}",
            unchecked_encoded(&payload)
        );
        assert!(verify_rule_share_url(&url)
            .unwrap_err()
            .to_string()
            .contains("invalid shared rule content"));
    }

    #[test]
    fn verify_checks_presence_duplicates_and_preserves_target() {
        assert!(verify_rule_share_url("https://example.com/")
            .unwrap_err()
            .to_string()
            .contains("Missing"));
        let payload = sample_payload();
        let url = append_rule_share_query("https://example.com/path?site=1#tab", &payload).unwrap();
        let parts = verify_rule_share_url(&url).unwrap();
        assert_eq!(parts.clean_url, "https://example.com/path?site=1#tab");
        assert_eq!(parts.payload, Some(payload));
        let duplicate = url.replace("#tab", "&__bifrost_rule=bad#tab");
        assert!(verify_rule_share_url(&duplicate)
            .unwrap_err()
            .to_string()
            .contains("Duplicate"));
        assert!(
            extract_rule_share_query("https://example.com/?other=__bifrost_rule")
                .unwrap()
                .payload
                .is_none()
        );
    }

    #[test]
    fn verify_rejects_malformed_and_unsupported_payloads() {
        for encoded in ["!", "e30", ""] {
            assert!(verify_rule_share_url(&format!(
                "https://example.com/?__bifrost_rule={encoded}"
            ))
            .is_err());
        }
        let mut payload = sample_payload();
        payload.version = 99;
        assert!(decode_rule_share_payload(&unchecked_encoded(&payload))
            .unwrap_err()
            .to_string()
            .contains("version"));
        payload.version = 1;
        payload.content_hash_algorithm = "md5".into();
        assert!(decode_rule_share_payload(&unchecked_encoded(&payload))
            .unwrap_err()
            .to_string()
            .contains("algorithm"));
        assert!(new_rule_share_payload("", "example.test status://200").is_err());
        assert!(new_rule_share_payload("bad/name", "example.test status://200").is_err());
        assert!(new_rule_share_payload("demo", "").is_err());
        assert!(verify_rule_share_url("ftp://example.com/?__bifrost_rule=x").is_err());
    }

    #[test]
    fn shared_syntax_keeps_inline_values_and_receiver_local_references() {
        let content = "@local\nexample.test resBody://{body}\n``` body\nhello\n```";
        assert!(new_rule_share_payload("demo", content).is_ok());
    }
    #[test]
    fn detects_encoded_share_keys_but_not_values_or_fragments() {
        assert!(has_rule_share_query(
            "https://example.test/?%5F%5Fbifrost_rule=bad"
        ));
        assert!(has_rule_share_query(
            "https://example.test/?__bifrost_rule=bad#tab"
        ));
        assert!(!has_rule_share_query(
            "https://example.test/?other=__bifrost_rule"
        ));
        assert!(!has_rule_share_query(
            "https://example.test/#__bifrost_rule"
        ));
        assert!(!has_rule_share_query(
            "https://example.test/?other=1#__bifrost_rule=x"
        ));
    }
}
