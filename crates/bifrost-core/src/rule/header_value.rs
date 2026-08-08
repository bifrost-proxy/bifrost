use serde_json::Value;

/// Parse a `reqHeaders://` or `resHeaders://` value into header pairs.
///
/// Single-line inline values accept `&` and `,` between headers. Multi-line
/// values deliberately split only on newlines so literal ampersands and commas
/// remain available inside a header value. JSON objects are parsed before any
/// delimiter handling for the same reason.
pub fn parse_rule_header_pairs(value: &str) -> Option<Vec<(String, String)>> {
    parse_header_pairs(value)
}

fn parse_header_pairs(value: &str) -> Option<Vec<(String, String)>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let content = strip_wrapping_parens(trimmed);
    if looks_like_json_header_object(content) {
        return Some(parse_json_header_object(content).unwrap_or_default());
    }

    let parts: Box<dyn Iterator<Item = &str>> = if content.contains('\n') {
        Box::new(content.lines())
    } else {
        Box::new(content.split([',', '&']))
    };

    let headers = parts
        .map(str::trim)
        .filter(|part| !part.is_empty() && !part.starts_with('#'))
        .filter_map(parse_header_pair)
        .collect::<Vec<_>>();

    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

fn strip_wrapping_parens(value: &str) -> &str {
    if value.starts_with('(') && value.ends_with(')') && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn parse_header_pair(value: &str) -> Option<(String, String)> {
    let split_pos = match (value.find('='), value.find(':')) {
        (Some(eq), Some(colon)) => eq.min(colon),
        (Some(eq), None) => eq,
        (None, Some(colon)) => colon,
        (None, None) => return None,
    };
    let key = value[..split_pos].trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value[split_pos + 1..].trim().to_string()))
}

fn parse_json_header_object(content: &str) -> Option<Vec<(String, String)>> {
    let json_value = serde_json::from_str::<Value>(content).ok()?;
    let object = json_value.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(key, value)| {
                if key.trim().is_empty() {
                    return None;
                }
                json_scalar_to_header_value(value).map(|value| (key.clone(), value))
            })
            .collect(),
    )
}

fn looks_like_json_header_object(content: &str) -> bool {
    let content = content.trim();
    if !(content.starts_with('{') && content.ends_with('}')) {
        return false;
    }
    let inner = content[1..content.len() - 1].trim_start();
    inner.is_empty() || inner.starts_with('"') || inner.contains(':')
}

fn json_scalar_to_header_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some(String::new()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_rule_header_pairs;

    #[test]
    fn rule_headers_split_ampersand_with_equals_inside_parentheses() {
        let headers = parse_rule_header_pairs(
            "(x-tt-env=ppe_doubao_connect_lark&x-flow-env=ppe_doubao_connect_lark&x-use-ppe=1)",
        )
        .expect("headers");

        assert_eq!(
            headers,
            vec![
                ("x-tt-env".into(), "ppe_doubao_connect_lark".into()),
                ("x-flow-env".into(), "ppe_doubao_connect_lark".into()),
                ("x-use-ppe".into(), "1".into()),
            ]
        );
    }

    #[test]
    fn rule_headers_keep_ampersand_in_multiline_and_json_values() {
        assert_eq!(
            parse_rule_header_pairs("X-Query: a=1&b=2\nX-Mode: test"),
            Some(vec![
                ("X-Query".into(), "a=1&b=2".into()),
                ("X-Mode".into(), "test".into()),
            ])
        );
        assert_eq!(
            parse_rule_header_pairs(r#"{"X-Query":"a=1&b=2","X-Mode":"test"}"#),
            Some(vec![
                ("X-Mode".into(), "test".into()),
                ("X-Query".into(), "a=1&b=2".into()),
            ])
        );
    }

    #[test]
    fn rule_headers_parse_json_scalar_values_and_skip_unsupported_entries() {
        let headers = parse_rule_header_pairs(
            r#"{"":"ignored","X-Number":42,"X-Bool":true,"X-Null":null,"X-Array":[1],"X-Object":{"nested":true}}"#,
        )
        .expect("headers");

        assert!(headers.contains(&("X-Number".into(), "42".into())));
        assert!(headers.contains(&("X-Bool".into(), "true".into())));
        assert!(headers.contains(&("X-Null".into(), String::new())));
        assert!(!headers.iter().any(|(name, _)| name.is_empty()));
        assert!(!headers.iter().any(|(name, _)| name == "X-Array"));
        assert!(!headers.iter().any(|(name, _)| name == "X-Object"));
    }

    #[test]
    fn rule_headers_preserve_single_header_and_ignore_malformed_parts() {
        assert_eq!(
            parse_rule_header_pairs("X-Single=value"),
            Some(vec![("X-Single".into(), "value".into())])
        );
        assert_eq!(
            parse_rule_header_pairs("missing&X-Valid=1&=ignored"),
            Some(vec![("X-Valid".into(), "1".into())])
        );
        assert_eq!(parse_rule_header_pairs(r#"{"X-Bad":}"#), Some(vec![]));
    }
}
